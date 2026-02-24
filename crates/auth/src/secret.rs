pub use inner::*;

#[derive(thiserror::Error, Debug)]
pub enum SecretStorageError {
    #[error("Access to the secret storage was denied")]
    AccessDenied,
    #[error("Serialization error")]
    SerializationError,
    #[error("I/O error")]
    IoError,
    #[error("Unknown error")]
    UnknownError,
    #[error("Not unique")]
    NotUnique,
    #[cfg(target_os = "windows")]
    #[error("Windows error: {0}")]
    WindowsError(#[from] windows::core::Error),
    #[cfg(target_os = "macos")]
    #[error("Security.Framework error: {0}")]
    SecurityFrameworkError(#[from] security_framework::base::Error),
}

#[cfg(target_os = "linux")]
mod inner {
    use uuid::Uuid;

    use crate::{credentials::AccountCredentials, secret::SecretStorageError};

    impl From<oo7::Error> for SecretStorageError {
        fn from(value: oo7::Error) -> Self {
            Self::from(&value)
        }
    }

    impl From<&oo7::Error> for SecretStorageError {
        fn from(value: &oo7::Error) -> Self {
            match value {
                oo7::Error::File(error) => match error {
                    oo7::file::Error::Io(_) => Self::IoError,
                    _ => Self::UnknownError,
                },
                oo7::Error::DBus(error) => match error {
                    oo7::dbus::Error::Service(service_error) => match service_error {
                        oo7::dbus::ServiceError::IsLocked(_) => Self::AccessDenied,
                        _ => Self::UnknownError,
                    },
                    oo7::dbus::Error::Dismissed => Self::AccessDenied,
                    oo7::dbus::Error::IO(_) => Self::IoError,
                    _ => Self::UnknownError,
                },
            }
        }
    }

    pub struct PlatformSecretStorage {
        keyring: oo7::Result<oo7::Keyring>,
    }

    impl PlatformSecretStorage {
        pub async fn new() -> Result<Self, SecretStorageError> {
            Ok(Self {
                keyring: oo7::Keyring::new().await,
            })
        }

        pub async fn read_credentials(&self, uuid: Uuid) -> Result<Option<AccountCredentials>, SecretStorageError> {
            let keyring = self.keyring.as_ref()?;
            keyring.unlock().await?;

            let uuid_str = uuid.as_hyphenated().to_string();
            let attributes = vec![("service", "pandora-launcher"), ("uuid", uuid_str.as_str())];

            let items = keyring.search_items(&attributes).await?;

            if items.is_empty() {
                Ok(None)
            } else if items.len() > 1 {
                Err(SecretStorageError::NotUnique)
            } else {
                let raw = items[0].secret().await?;
                Ok(Some(serde_json::from_slice(&raw).map_err(|_| SecretStorageError::SerializationError)?))
            }
        }

        pub async fn write_credentials(
            &self,
            uuid: Uuid,
            credentials: &AccountCredentials,
        ) -> Result<(), SecretStorageError> {
            let keyring = self.keyring.as_ref()?;
            keyring.unlock().await?;

            let uuid_str = uuid.as_hyphenated().to_string();
            let attributes = vec![("service", "pandora-launcher"), ("uuid", uuid_str.as_str())];

            let bytes = serde_json::to_vec(credentials).map_err(|_| SecretStorageError::SerializationError)?;

            keyring.create_item("Pandora Minecraft Account", &attributes, bytes, true).await?;
            Ok(())
        }

        pub async fn delete_credentials(&self, uuid: Uuid) -> Result<(), SecretStorageError> {
            let keyring = self.keyring.as_ref()?;
            keyring.unlock().await?;

            let uuid_str = uuid.as_hyphenated().to_string();
            let attributes = vec![("service", "pandora-launcher"), ("uuid", uuid_str.as_str())];

            keyring.delete(&attributes).await?;
            Ok(())
        }
    }
}

#[cfg(target_os = "windows")]
mod inner {
    use std::path::PathBuf;

    use uuid::Uuid;

    use crate::{credentials::AccountCredentials, secret::SecretStorageError};

    use windows::Win32::Security::Credentials::*;

    /// Windows Credential Manager limits each blob to 2560 bytes. Our tokens (e.g. JWT) can exceed this.
    const CRED_MAX_BLOB_SIZE: usize = 2560;

    fn credentials_dir() -> Result<PathBuf, SecretStorageError> {
        let appdata = std::env::var("APPDATA").map_err(|_| SecretStorageError::UnknownError)?;
        Ok(PathBuf::from(appdata).join("PandoraLauncher").join("credentials"))
    }

    fn credential_file_path(uuid: Uuid) -> Result<std::path::PathBuf, SecretStorageError> {
        Ok(credentials_dir()?.join(format!("{}.json", uuid.as_hyphenated())))
    }

    pub struct PlatformSecretStorage;

    impl PlatformSecretStorage {
        pub async fn new() -> Result<Self, SecretStorageError> {
            Ok(Self)
        }

        pub async fn read_credentials(&self, uuid: Uuid) -> Result<Option<AccountCredentials>, SecretStorageError> {
            // Try file first (used when Credential Manager failed due to size/admin)
            if let Ok(path) = credential_file_path(uuid) {
                if path.exists() {
                    if let Ok(data) = tokio::fs::read(&path).await {
                        if let Ok(creds) = serde_json::from_slice::<AccountCredentials>(&data) {
                            return Ok(Some(creds));
                        }
                    }
                }
            }

            // Fall back to Credential Manager
            fn read_cm<T: for<'a> serde::Deserialize<'a>>(target: String) -> Result<Option<T>, SecretStorageError> {
                let mut target_name: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();

                let mut credentials: *mut CREDENTIALW = std::ptr::null_mut();

                unsafe {
                    let result = CredReadW(
                        windows::core::PWSTR::from_raw(target_name.as_mut_ptr()),
                        CRED_TYPE_GENERIC,
                        None,
                        &mut credentials,
                    );

                    if let Err(error) = result {
                        const ERROR_NOT_FOUND: windows::core::HRESULT =
                            windows::core::HRESULT::from_win32(windows::Win32::Foundation::ERROR_NOT_FOUND.0);
                        if error.code() == ERROR_NOT_FOUND {
                            return Ok(None);
                        }
                        return Err(error.into());
                    }

                    let Some(credentials) = credentials.as_mut() else {
                        return Ok(None);
                    };

                    let raw =
                        std::slice::from_raw_parts(credentials.CredentialBlob, credentials.CredentialBlobSize as usize);
                    Ok(Some(serde_json::from_slice(&raw).map_err(|_| SecretStorageError::SerializationError)?))
                }
            }

            let mut account = AccountCredentials::default();
            let uuid_fmt = uuid.as_hyphenated();
            account.msa_refresh = read_cm(format!("PandoraLauncher_MsaRefresh_{}", uuid_fmt))?;
            account.msa_refresh_force_client_id =
                read_cm(format!("PandoraLauncher_MsaRefreshForceClientId_{}", uuid_fmt))?;
            account.msa_access = read_cm(format!("PandoraLauncher_MsaAccess_{}", uuid_fmt))?;
            account.xbl = read_cm(format!("PandoraLauncher_Xbl_{}", uuid_fmt))?;
            account.xsts = read_cm(format!("PandoraLauncher_Xsts_{}", uuid_fmt))?;
            account.access_token = read_cm(format!("PandoraLauncher_AccessToken_{}", uuid_fmt))?;

            Ok(Some(account))
        }

        pub async fn write_credentials(
            &self,
            uuid: Uuid,
            credentials: &AccountCredentials,
        ) -> Result<(), SecretStorageError> {
            fn write_cm_inner(target: String, bytes: Option<Vec<u8>>) -> Result<(), SecretStorageError> {
                let mut target_name: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();

                if let Some(mut bytes) = bytes {
                    if bytes.len() > CRED_MAX_BLOB_SIZE {
                        return Err(SecretStorageError::UnknownError);
                    }
                    let cred = CREDENTIALW {
                        Flags: CRED_FLAGS(0),
                        Type: CRED_TYPE_GENERIC,
                        TargetName: windows::core::PWSTR::from_raw(target_name.as_mut_ptr()),
                        CredentialBlobSize: bytes.len() as u32,
                        CredentialBlob: bytes.as_mut_ptr(),
                        Persist: CRED_PERSIST_SESSION,
                        ..CREDENTIALW::default()
                    };
                    unsafe { CredWriteW(&cred, 0)? };
                    Ok(())
                } else {
                    unsafe {
                        CredDeleteW(
                            windows::core::PWSTR::from_raw(target_name.as_mut_ptr()),
                            CRED_TYPE_GENERIC,
                            None,
                        )?;
                    }
                    Ok(())
                }
            }

            fn write_cm(target: String, data: Option<&impl serde::Serialize>) -> Result<(), SecretStorageError> {
                let bytes = data
                    .map(|v| serde_json::to_vec(v).map_err(|_| SecretStorageError::SerializationError))
                    .transpose()?;
                write_cm_inner(target, bytes)
            }

            let uuid_fmt = uuid.as_hyphenated();
            let cm_result = (|| -> Result<(), SecretStorageError> {
                write_cm(format!("PandoraLauncher_MsaRefresh_{}", uuid_fmt), credentials.msa_refresh.as_ref())?;
                write_cm(
                    format!("PandoraLauncher_MsaRefreshForceClientId_{}", uuid_fmt),
                    credentials.msa_refresh_force_client_id.as_ref(),
                )?;
                write_cm(format!("PandoraLauncher_MsaAccess_{}", uuid_fmt), credentials.msa_access.as_ref())?;
                write_cm(format!("PandoraLauncher_Xbl_{}", uuid_fmt), credentials.xbl.as_ref())?;
                write_cm(format!("PandoraLauncher_Xsts_{}", uuid_fmt), credentials.xsts.as_ref())?;
                write_cm(format!("PandoraLauncher_AccessToken_{}", uuid_fmt), credentials.access_token.as_ref())?;
                Ok(())
            })();

            if let Err(_) = cm_result {
                // Credential Manager failed (admin, blob size, etc.). Store in file under APPDATA.
                let dir = credentials_dir()?;
                tokio::fs::create_dir_all(&dir).await.map_err(|_| SecretStorageError::IoError)?;
                let path = credential_file_path(uuid)?;
                let bytes = serde_json::to_vec(credentials).map_err(|_| SecretStorageError::SerializationError)?;
                tokio::fs::write(&path, &bytes).await.map_err(|_| SecretStorageError::IoError)?;
            }

            Ok(())
        }

        pub async fn delete_credentials(&self, uuid: Uuid) -> Result<(), SecretStorageError> {
            if let Ok(path) = credential_file_path(uuid) {
                let _ = tokio::fs::remove_file(&path).await;
            }

            fn delete_cm(target: String) -> windows::core::Result<()> {
                let mut target_name: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
                unsafe {
                    CredDeleteW(windows::core::PWSTR::from_raw(target_name.as_mut_ptr()), CRED_TYPE_GENERIC, None)
                }
            }

            let uuid_fmt = uuid.as_hyphenated();
            let _ = delete_cm(format!("PandoraLauncher_MsaRefresh_{}", uuid_fmt));
            let _ = delete_cm(format!("PandoraLauncher_MsaRefreshForceClientId_{}", uuid_fmt));
            let _ = delete_cm(format!("PandoraLauncher_MsaAccess_{}", uuid_fmt));
            let _ = delete_cm(format!("PandoraLauncher_Xbl_{}", uuid_fmt));
            let _ = delete_cm(format!("PandoraLauncher_Xsts_{}", uuid_fmt));
            let _ = delete_cm(format!("PandoraLauncher_AccessToken_{}", uuid_fmt));

            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
mod inner {
    use security_framework::os::macos::keychain::{SecKeychain, SecPreferencesDomain};
    use uuid::Uuid;

    use crate::{credentials::AccountCredentials, secret::SecretStorageError};

    pub struct PlatformSecretStorage {
        keychain: SecKeychain,
    }

    impl PlatformSecretStorage {
        pub async fn new() -> Result<Self, SecretStorageError> {
            Ok(Self {
                keychain: SecKeychain::default_for_domain(SecPreferencesDomain::User)?
            })
        }

        pub async fn read_credentials(&self, uuid: Uuid) -> Result<Option<AccountCredentials>, SecretStorageError> {
            let uuid_str = uuid.as_hyphenated().to_string();
            let data = match self.keychain.find_generic_password("com.moulberry.pandoralauncher", uuid_str.as_str()) {
                Ok((data, _)) => data,
                Err(error) if error.code() == security_framework_sys::base::errSecItemNotFound => {
                    return Ok(None);
                },
                Err(error) => {
                    return Err(error.into());
                }
            };
            let data = data.as_ref();
            Ok(Some(serde_json::from_slice(&data).map_err(|_| SecretStorageError::SerializationError)?))
        }

        pub async fn write_credentials(
            &self,
            uuid: Uuid,
            credentials: &AccountCredentials,
        ) -> Result<(), SecretStorageError> {
            let uuid_str = uuid.as_hyphenated().to_string();
            let bytes = serde_json::to_vec(credentials).map_err(|_| SecretStorageError::SerializationError)?;

            self.keychain.set_generic_password("com.moulberry.pandoralauncher", uuid_str.as_str(), &bytes)?;
            Ok(())
        }

        pub async fn delete_credentials(&self, uuid: Uuid) -> Result<(), SecretStorageError> {
            let uuid_str = uuid.as_hyphenated().to_string();

            let item = match self.keychain.find_generic_password("com.moulberry.pandoralauncher", uuid_str.as_str()) {
                Ok((_, item)) => item,
                Err(error) if error.code() == security_framework_sys::base::errSecItemNotFound => {
                    return Ok(());
                },
                Err(error) => {
                    return Err(error.into());
                }
            };

            item.delete();
            Ok(())
        }
    }
}
