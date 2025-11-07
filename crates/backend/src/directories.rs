use std::path::PathBuf;



pub struct LauncherDirectories {
    pub instances_dir: PathBuf,
    
    pub metadata_dir: PathBuf,
    
    pub assets_root_dir: PathBuf,
    pub assets_index_dir: PathBuf,
    pub assets_objects_dir: PathBuf,
    pub virtual_legacy_assets_dir: PathBuf,
    
    pub libraries_dir: PathBuf,
    pub log_configs_dir: PathBuf,
    pub runtime_base_dir: PathBuf,
    
    pub temp_dir: PathBuf,
    pub temp_natives_base_dir: PathBuf,
    
    pub accounts_json: PathBuf,
    pub accounts_json_backup: PathBuf,
}

impl LauncherDirectories {
    pub fn new(launcher_dir: PathBuf) -> Self {
        let instances_dir = launcher_dir.join("instances");
        
        let metadata_dir = launcher_dir.join("metadata");
        
        let assets_root_dir = launcher_dir.join("assets");
        let assets_index_dir = assets_root_dir.join("indexes");
        let assets_objects_dir = assets_root_dir.join("objects");
        let virtual_legacy_assets_dir = assets_index_dir.join("virtual").join("legacy");
        
        let libraries_dir = launcher_dir.join("libraries");
        
        let log_configs_dir = launcher_dir.join("logconfigs");

        let runtime_base_dir = launcher_dir.join("runtime");

        let temp_dir = launcher_dir.join("temp");
        let temp_natives_base_dir = temp_dir.join("natives");
        
        let accounts_json = launcher_dir.join("accounts.json");
        let accounts_json_backup = launcher_dir.join("accounts.json.old");
        
        Self {
            instances_dir,
            
            metadata_dir,
            
            assets_root_dir,
            assets_index_dir,
            assets_objects_dir,
            virtual_legacy_assets_dir,
            
            libraries_dir,
            log_configs_dir,
            runtime_base_dir,
            
            temp_dir,
            temp_natives_base_dir,
            
            accounts_json,
            accounts_json_backup,
        }
    }
}
