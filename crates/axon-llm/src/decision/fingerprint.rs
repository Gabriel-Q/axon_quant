//! 请求规范化与指纹：Replay 后端的精确匹配键（跨语言单一真源）
//!
//! canonical = 紧凑 JSON（键排序来自 BTreeMap、无空格、UTF-8 原文），
//! 与 QuantCell `json.dumps(ensure_ascii=False, sort_keys=True, separators=(',',':'))` 等价。

use sha2::{Digest, Sha256};

use super::types::DecisionRequest;

/// 规范化请求体（紧凑、键有序、UTF-8）
pub fn canonical_request(req: &DecisionRequest) -> String {
    // DecisionRequest 仅含 Value/BTreeMap/字符串/数字，序列化不可能失败
    serde_json::to_string(req).expect("decision request is always serializable")
}

/// 请求指纹（sha256 hex）
pub fn request_fingerprint(req: &DecisionRequest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(canonical_request(req).as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn req() -> DecisionRequest {
        let raw = json!({
            "state": {"b": 2, "a": 1},
            "questions": {
                "q": {"type": "noul", "instructions": "是否？"}
            }
        });
        serde_json::from_value(raw).unwrap()
    }

    #[test]
    fn canonical_sorts_keys_and_emits_utf8() {
        let c = canonical_request(&req());
        // state 的键有序：a 在 b 前；中文不转义
        assert!(c.contains("\"state\":{\"a\":1,\"b\":2}"), "got {c}");
        assert!(c.contains("是否？"), "got {c}");
        assert!(!c.contains(' '), "compact JSON must contain no spaces: {c}");
    }

    #[test]
    fn fingerprint_is_stable_64_hex() {
        let fp1 = request_fingerprint(&req());
        let fp2 = request_fingerprint(&req());
        assert_eq!(fp1, fp2);
        assert_eq!(fp1.len(), 64);
        assert!(fp1.chars().all(|c| c.is_ascii_hexdigit()));
        // BTreeMap 保证与插入顺序无关：换一个等价对象指纹相同
        let mut q = BTreeMap::new();
        q.insert(
            "q".to_string(),
            serde_json::from_value(json!({"type":"noul","instructions":"是否？"})).unwrap(),
        );
        let other = DecisionRequest {
            state: json!({"a": 1, "b": 2}),
            questions: q,
        };
        assert_eq!(fp1, request_fingerprint(&other));
    }
}
