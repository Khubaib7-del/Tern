//! The run-state contract's wire form (#85, A0): the fixtures front ends
//! build against parse into the core types and write back unchanged.

use tern_core::{
    ContextTrimmed, ErrorClass, ModelSwitched, PausedBy, RunState, TurnDigest, TurnReceipt,
};

fn round_trip<T>(text: &str) -> T
where
    T: serde::de::DeserializeOwned + serde::Serialize + PartialEq + std::fmt::Debug,
{
    let value: T = serde_json::from_str(text).expect("fixture parses");
    let again: T = serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
    assert_eq!(again, value, "writes back unchanged");
    let want: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        serde_json::to_value(&value).unwrap(),
        want,
        "same wire form"
    );
    value
}

#[test]
fn receipts_parse() {
    let r: TurnReceipt = round_trip(include_str!(
        "fixtures/run_contract/turn_receipt_daily.json"
    ));
    assert_eq!(r.state, RunState::Paused);
    assert_eq!(r.paused_by, Some(PausedBy::DailyQuota));
    assert_eq!(r.error, Some(ErrorClass::RateLimitDaily));
    assert_eq!(
        r.kept.line(),
        "Kept: 412 characters, 2 sources, 7 tool results"
    );
    let u: TurnReceipt = round_trip(include_str!("fixtures/run_contract/turn_receipt_user.json"));
    assert_eq!(u.paused_by, Some(PausedBy::User));
    assert_eq!(u.line, PausedBy::User.line());
}

#[test]
fn switches_trims_and_digests_parse() {
    let s: ModelSwitched = round_trip(include_str!("fixtures/run_contract/model_switched.json"));
    assert!(!s.chosen_by_user);
    let t: ContextTrimmed = round_trip(include_str!("fixtures/run_contract/context_trimmed.json"));
    assert!(t.after_tokens < t.before_tokens);
    let d: TurnDigest = round_trip(include_str!("fixtures/run_contract/turn_digest.json"));
    assert!(d.history_text().contains("paper_read ×2"));
}

#[test]
fn kinds_are_dotted_names() {
    for k in [
        tern_core::run_state::TURN_RECEIPT,
        tern_core::run_state::MODEL_SWITCHED,
        tern_core::run_state::CONTEXT_TRIMMED,
        tern_core::run_state::TURN_DIGEST,
    ] {
        assert!(
            k.contains('.')
                && k.chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_')
        );
    }
}
