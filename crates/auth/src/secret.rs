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
                    _ => Self::UnknownError
                },
                oo7::Error::DBus(error) => match error {
                    oo7::dbus::Error::Service(service_error) => match service_error {
                        oo7::dbus::ServiceError::IsLocked(_) => Self::AccessDenied,
                        _ => Self::UnknownError,
                    },
                    oo7::dbus::Error::Dismissed => Self::AccessDenied,
                    oo7::dbus::Error::IO(_) => Self::IoError,
                    _ => Self::UnknownError
                },
            }
        }
    }
    
    pub struct PlatformSecretStorage {
        keyring: oo7::Result<oo7::Keyring>,
    }
    
    impl PlatformSecretStorage {
        pub async fn new() -> Self {
            Self {
                keyring: oo7::Keyring::new().await
            }
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
        
        pub async fn write_credentials(&self, uuid: Uuid, credentials: &AccountCredentials) -> Result<(), SecretStorageError> {
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
