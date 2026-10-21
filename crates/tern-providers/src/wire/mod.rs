//! Request bodies and response parsing per wire format. Pure functions, so
//! they are tested against recorded JSON shapes without a network.

pub(crate) mod anthropic;
pub(crate) mod gemini;
pub(crate) mod openai;

use serde_json::Value;

pub(crate) fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter().try_fold(v, |v, k| v.get(*k))?.as_str()
}

pub(crate) fn u64_at(v: &Value, path: &[&str]) -> Option<u64> {
    path.iter().try_fold(v, |v, k| v.get(*k))?.as_u64()
}
