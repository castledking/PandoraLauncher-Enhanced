#!/bin/bash
set -e

if [ -z "$1" ]; then
    echo "Missing version argument"
    exit 1
fi

version=${1#v}
export PANDORA_RELEASE_VERSION=$version

cargo build --release --frozen --target x86_64-pc-windows-msvc
strip target/x86_64-pc-windows-msvc/release/pandora_launcher.exe

# The MSVC C runtime must be linked statically (see .cargo/config.toml), otherwise
# the binary imports VCRUNTIME140.dll and fails to start on machines without the
# Visual C++ Redistributable installed.
if grep -aq 'VCRUNTIME140[^ ]*\.dll' target/x86_64-pc-windows-msvc/release/pandora_launcher.exe; then
    echo "error: binary imports VCRUNTIME140.dll; the MSVC runtime was linked dynamically" >&2
    exit 1
fi

mkdir -p dist

mv target/x86_64-pc-windows-msvc/release/pandora_launcher dist/PandoraLauncher-Windows-x86_64.exe

cargo install cargo-packager
env -u CARGO_PACKAGER_SIGN_PRIVATE_KEY cargo packager --config '{'\
'  "name": "pandora-launcher",'\
'  "outDir": "./dist",'\
'  "productName": "Pandora Launcher",'\
'  "version": "'"$version"'",'\
'  "identifier": "com.moulberry.pandoralauncher",'\
'  "resources": [],'\
'  "authors": ["Moulberry"],'\
'  "binaries": [{ "path": "PandoraLauncher-Windows-x86_64.exe", "main": true }],'\
'  "icons": ["package/windows.ico"]'\
'}'

mv -f dist/PandoraLauncher-Windows-x86_64.exe dist/PandoraLauncher-Windows-x86_64-Portable.exe
mv -f 'dist/PandoraLauncher-Windows-x86_64_'$version'_x64-setup.exe' dist/PandoraLauncher-Windows-x86_64-Setup.exe

if [[ -n "$CARGO_PACKAGER_SIGN_PRIVATE_KEY" ]]; then
    cargo packager signer sign dist/PandoraLauncher-Windows-x86_64-Portable.exe

    GITHUB_REPO="${GITHUB_REPOSITORY_URL:-https://github.com/Moulberry/PandoraLauncher}"
    echo "{
    \"version\": \"$version\",
    \"downloads\": {
        \"x86_64\": {
            \"executable\": {
                \"download\": \"$GITHUB_REPO/releases/download/v$version/PandoraLauncher-Windows-x86_64-Portable.exe\",
                \"size\": $(wc -c < dist/PandoraLauncher-Windows-x86_64-Portable.exe),
                \"sha1\": \"$(sha1sum dist/PandoraLauncher-Windows-x86_64-Portable.exe | cut -d ' ' -f 1)\",
                \"sig\": \"$(cat dist/PandoraLauncher-Windows-x86_64-Portable.exe.sig)\"
            }
        }
    }
}" > dist/update_windows.json
    cp dist/update_windows.json dist/update_manifest_windows.json

    rm dist/*.sig
fi
