# Pandora Launcher

Work in progress

## Features
- Instance management
- Secure account credential management using platform keyrings
- Custom game output window
- Mod browser using Modrinth's API
- Automatic redaction of sensitive information (i.e. access tokens) in logs

## Comparison between Pandora and other launchers

In addition to the features below, each launcher also has different performance and design characteristics. For example, Prism takes 4 clicks to create a Fabric instance (New Instance > Edit Instance > Install Fabric > Confirm), whereas Pandora and Modrinth allow you to install Fabric directly on the create instance page. These design characteristics are subjective and you should form your own opinion on what design works best for you. Performance is similarly complex, and you should experiment with different launchers if performance is a priority for you.

(If the below information is inaccurate/out-of-date, please let me know :))

Please keep in mind that Pandora *is* a work in progress, and many of the things below will be added as it develops

|   | Pandora | Official | MultiMC/Prism | Modrinth |
|---|---|---|---|---|
|Frontend|GPUI (Native, GPU)|?|Qt (Native)|Tauri (Web)|
|No Ads/Cosmetics|✔|✔|✔|-|
|Supports mod devs|-|-|-|✔|
|Content sources|-|-|-|Modrinth|
|Game output|~~Filter~~(wip), search, ~~upload~~(wip)|Filter, search|Search (forwards-only)|Filter, search, upload|
|Secure credentials|[✔](## "Uses platform keyrings (Windows Credential Store, MacOS Keychain, Linux Secret Service)")|[-](## "Uses platform keyrings (Windows Credential Store, MacOS Keychain, Linux Secret Service), but leaks access tokens in game launch arguments")|[-](## "Leaks account credentials in logs and stores them in plain-text on the disk")|[-](## "Stores account credentials using Tauri localstorage, harder to access than plain-text")|
|Fabric|✔|-|✔|✔|
|(Neo)forge|-|-|✔|✔|
|Modpack support|-|-|CF, MR, etc.|MR|

## Instance Page
![Instance Page](https://raw.githubusercontent.com/Moulberry/PandoraLauncher/refs/heads/master/screenshots/instance.png)
