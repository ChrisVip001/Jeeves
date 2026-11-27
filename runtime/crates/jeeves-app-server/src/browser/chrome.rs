//! Locate Chrome's user-approved loopback debugging target without opening its profile.
use anyhow::{Result, anyhow, ensure};

fn parse_endpoint(contents: &str) -> Result<String> {
    let mut lines = contents.lines().map(str::trim);
    let port = lines
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or_else(|| anyhow!("Chrome reported an invalid debugging port"))?;
    let target = lines
        .next()
        .and_then(|value| value.strip_prefix("/devtools/browser/"))
        .filter(|id| {
            id.len() == 36
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        })
        .ok_or_else(|| anyhow!("Chrome reported an invalid browser endpoint"))?;
    Ok(format!("ws://127.0.0.1:{port}/devtools/browser/{target}"))
}
pub(super) async fn endpoint(directory: &std::path::Path) -> Result<String> {
    use tokio::io::AsyncReadExt;

    let reminder = "Open Chrome 144 or newer, enable remote debugging at chrome://inspect/#remote-debugging, and allow Jeeves when Chrome asks";
    let mut contents = String::new();
    let size = tokio::fs::File::open(directory.join("DevToolsActivePort"))
        .await
        .map_err(|_| anyhow!("{reminder}"))?
        .take(257)
        .read_to_string(&mut contents)
        .await
        .map_err(|_| anyhow!("{reminder}"))?;
    ensure!(size <= 256, "Chrome reported an invalid browser endpoint");
    parse_endpoint(&contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_debug_target_is_a_literal_loopback_websocket() {
        let path = "/devtools/browser/71a4d970-68c8-4000-a130-ba36d92ee2d1";
        assert_eq!(
            parse_endpoint(&format!("53137\n{path}\n")).unwrap(),
            format!("ws://127.0.0.1:53137{path}"),
        );
        assert!(parse_endpoint(&format!("0\n{path}")).is_err());
        assert!(parse_endpoint("53137\n//example.com/devtools/browser/x").is_err());
        assert!(parse_endpoint(&format!("53137\n{path}?cookie=value")).is_err());
    }
}
