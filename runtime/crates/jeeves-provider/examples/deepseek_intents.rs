//! Opt-in one-request smoke. Arguments: Jeeves home and the user's Flash profile ID.
use jeeves_config::models::{CredentialSlot, SecretStore, SecretVault, read_models};
use jeeves_provider::{CreateRequest, Message, Provider, ProviderError, openai::Client};
use serde_json::{Value, json};
use std::{
    env,
    path::Path,
    time::{Duration, Instant},
};

#[tokio::main]
async fn main() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    let [home, id] = args.as_slice() else {
        return Err("Pass the Jeeves home and a configured DeepSeek Flash profile ID".into());
    };
    let home = Path::new(home);
    let models = read_models(home).map_err(|_| "Cannot read the Jeeves profile list")?;
    let profile = models
        .profiles
        .iter()
        .find(|p| {
            &p.id == id && p.provider == "deepseek" && p.model.as_deref() == Some("deepseek-flash")
        })
        .ok_or("No matching DeepSeek Flash profile; 0 requests")?;
    let base = profile
        .base_url
        .as_deref()
        .unwrap_or("https://api.deepseek.com")
        .trim_end_matches('/');
    if !matches!(
        base,
        "https://api.deepseek.com" | "https://api.deepseek.com/v1"
    ) {
        return Err(
            "The selected profile does not use the official DeepSeek endpoint; 0 requests".into(),
        );
    }
    let key = env::var("DEEPSEEK_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| match profile.key_source.as_deref()? {
            "keychain" | "file" => {
                let slot: CredentialSlot = profile.credential_slot;
                SecretStore::auto()
                    .versioned_credential(home, &slot.account(id))
                    .map(|(key, _)| key)
            }
            source => env::var(source.strip_prefix("env:")?).ok(),
        })
        .filter(|key| !key.trim().is_empty())
        .ok_or("The configured DeepSeek credential is unavailable; 0 requests")?;
    let client =
        Client::new(&key, Some(base), Duration::from_secs(45), 0).with_reasoning_content(true);
    let request = CreateRequest {
        model: "deepseek-flash".into(), cache_prefix: None,
        system: Some("Classify each independent message as chat, create, continue, or clarify. Return JSON only, with a rows array of [intent,existing task ID or null] pairs in input order. A discussion is chat. Create only for a new assignment. Ambiguous references require clarification.".into()),
        messages: vec![Message::user_text("1. No tasks. 你好，今天聊点什么？\n2. No tasks. 帮我整理一份开源发布检查清单。\n3. Open T17 and T42. 把任务 T42 的推广文案改成草稿。\n4. Two equally recent draft announcements T17 and T42; no other context. 刚才那个先别发，改成草稿。")],
        tools: vec![], max_tokens: 600,
    };
    let start = Instant::now();
    eprintln!("Starting 1 DeepSeek chat completion, retries disabled");
    let answer = client.create(request).await.map_err(|error| match error {
        ProviderError::Http { status, .. } | ProviderError::ContextOverflow { status, .. } => {
            format!("DeepSeek returned HTTP {status}")
        }
        ProviderError::Network(e) if e.is_timeout() => "Request timed out".into(),
        ProviderError::Network(_) => "Unable to reach the official endpoint".into(),
        _ => "The response could not be decoded".into(),
    })?;
    let parsed: Value =
        serde_json::from_str(&answer.text()).map_err(|_| "Reply was not plain JSON")?;
    let expected = json!([
        ["chat", null],
        ["create", null],
        ["continue", "T42"],
        ["clarify", null]
    ]);
    if parsed["rows"] != expected {
        return Err("Intent or target differed from the four expected synthetic decisions".into());
    }
    println!(
        "PASS model=deepseek-flash requests=1 synthetic_intents=4 input_tokens={} output_tokens={} elapsed_ms={}",
        answer.usage.prompt_size(),
        answer.usage.output_tokens,
        start.elapsed().as_millis()
    );
    Ok(())
}
