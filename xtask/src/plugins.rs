pub trait TaskPlugin {
    fn new(args: &[String], config: &toml::Value) -> Self
    where
        Self: Sized;
    fn description() -> &'static str
    where
        Self: Sized;
    fn execute(&self) -> crate::utils::TaskResult<()>;
}

// Include all generated plugin modules
include!(concat!(env!("OUT_DIR"), "/generated_mods.rs"));
