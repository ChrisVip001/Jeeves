//! Optional, explicitly installed browser runtime. Read only during Settings::from_env.
use std::path::PathBuf;
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserRuntime {
    pub root: PathBuf,
    pub headless: bool,
    pub cdp_attach: bool,
    pub chrome_data_dir: Option<PathBuf>,
}
pub(super) fn from_env() -> Option<BrowserRuntime> {
    super::env("JEEVES_BROWSER_RUNTIME").map(|root| BrowserRuntime {
        root: PathBuf::from(root),
        headless: super::env_bool("JEEVES_BROWSER_HEADLESS"),
        cdp_attach: super::env_bool("JEEVES_BROWSER_CDP_ATTACH"),
        chrome_data_dir: super::env("JEEVES_BROWSER_CHROME_DATA_DIR").map(PathBuf::from),
    })
}
