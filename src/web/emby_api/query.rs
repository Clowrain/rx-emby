use axum::{
    async_trait,
    extract::{FromRequestParts, Query},
    http::{request::Parts, StatusCode, Uri},
};
use serde::de;
use std::fmt;

/// Emby 规范化的 Query 参数提取器：
/// 1. 键名大小写不敏感（对齐 Emby Server / ASP.NET Core 行为）
/// 2. Token 别名（`api_key` / `ApiKey` / `X-Emby-Token` / `X-MediaBrowser-Token`）统一归一并去重，
///    避免客户端同时附带多个认证参数时触发 `serde` `duplicate field` 400 错误
/// 3. 同名/同义参数保留首个非空值；未知参数静默忽略
/// 4. 保留结构体原有 `#[serde(rename = "...")]` 定义，不影响共用 DTO 的 `Json<T>` 反序列化
#[derive(Debug, Clone, Copy, Default)]
pub(in crate::web::emby_api) struct EmbyQuery<T>(pub T);

#[async_trait]
impl<S, T> FromRequestParts<S> for EmbyQuery<T>
where
    S: Send + Sync,
    T: de::DeserializeOwned,
{
    type Rejection = (StatusCode, String);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let raw = parts.uri.query().unwrap_or("");
        parse_emby_query(raw).map(EmbyQuery)
    }
}

pub(in crate::web::emby_api) fn parse_emby_query<T: de::DeserializeOwned>(
    raw_query: &str,
) -> Result<T, (StatusCode, String)> {
    let fields = collect_struct_fields::<T>();
    if fields.is_empty() {
        return deserialize_query_str(raw_query);
    }

    let mut deduped: Vec<(&'static str, String)> = Vec::new();
    for (raw_key, raw_val) in url::form_urlencoded::parse(raw_query.as_bytes()) {
        let Some(canonical) = resolve_field_name(fields, &raw_key) else {
            continue;
        };

        if let Some((_, existing)) = deduped.iter_mut().find(|(k, _)| *k == canonical) {
            if existing.trim().is_empty() && !raw_val.trim().is_empty() {
                *existing = raw_val.into_owned();
            }
        } else {
            deduped.push((canonical, raw_val.into_owned()));
        }
    }

    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in &deduped {
        serializer.append_pair(k, v);
    }
    let normalized = serializer.finish();

    deserialize_query_str(&normalized)
}

fn deserialize_query_str<T: de::DeserializeOwned>(
    query: &str,
) -> Result<T, (StatusCode, String)> {
    let uri_str = if query.is_empty() {
        "/".to_string()
    } else {
        format!("/?{query}")
    };
    let uri: Uri = uri_str.parse().map_err(|err| {
        (
            StatusCode::BAD_REQUEST,
            format!("Invalid query string: {err}"),
        )
    })?;
    Query::<T>::try_from_uri(&uri)
        .map(|Query(inner)| inner)
        .map_err(|rejection| (StatusCode::BAD_REQUEST, rejection.body_text()))
}

fn is_token_key(key: &str) -> bool {
    key.eq_ignore_ascii_case("api_key")
        || key.eq_ignore_ascii_case("apikey")
        || key.eq_ignore_ascii_case("x-emby-token")
        || key.eq_ignore_ascii_case("x-mediabrowser-token")
}

fn resolve_field_name(fields: &'static [&'static str], raw_key: &str) -> Option<&'static str> {
    if is_token_key(raw_key) {
        return fields
            .iter()
            .copied()
            .find(|&f| f.eq_ignore_ascii_case("api_key") || is_token_key(f));
    }

    if let Some(exact_ci) = fields.iter().copied().find(|&f| f.eq_ignore_ascii_case(raw_key)) {
        return Some(exact_ci);
    }

    // 兜底：兼容结构体未写 rename 的 snake_case 字段与 PascalCase/camelCase 参数匹配
    let compact_raw: String = raw_key.chars().filter(|c| *c != '_').collect();
    fields.iter().copied().find(|&f| {
        let mut raw_chars = compact_raw.chars().map(|c| c.to_ascii_lowercase());
        let mut field_chars = f
            .chars()
            .filter(|c| *c != '_')
            .map(|c| c.to_ascii_lowercase());
        loop {
            match (raw_chars.next(), field_chars.next()) {
                (Some(a), Some(b)) if a == b => continue,
                (None, None) => return true,
                _ => return false,
            }
        }
    })
}

fn collect_struct_fields<T: de::DeserializeOwned>() -> &'static [&'static str] {
    let mut collector = FieldCollector { fields: &[] };
    let _ = T::deserialize(&mut collector);
    collector.fields
}

struct FieldCollector {
    fields: &'static [&'static str],
}

#[derive(Debug)]
struct CollectAbort;

impl fmt::Display for CollectAbort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("field collection aborted")
    }
}

impl std::error::Error for CollectAbort {}

impl de::Error for CollectAbort {
    fn custom<T: fmt::Display>(_msg: T) -> Self {
        Self
    }
}

macro_rules! abort_deserialize {
    ($($method:ident($($arg:ident : $ty:ty),*)),* $(,)?) => {
        $(
            fn $method<V: de::Visitor<'de>>(self, $($arg: $ty,)* _visitor: V) -> Result<V::Value, Self::Error> {
                Err(CollectAbort)
            }
        )*
    };
}

impl<'de> de::Deserializer<'de> for &mut FieldCollector {
    type Error = CollectAbort;

    fn deserialize_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.fields = fields;
        Err(CollectAbort)
    }

    fn deserialize_newtype_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    abort_deserialize!(
        deserialize_any(),
        deserialize_bool(),
        deserialize_i8(),
        deserialize_i16(),
        deserialize_i32(),
        deserialize_i64(),
        deserialize_u8(),
        deserialize_u16(),
        deserialize_u32(),
        deserialize_u64(),
        deserialize_f32(),
        deserialize_f64(),
        deserialize_char(),
        deserialize_str(),
        deserialize_string(),
        deserialize_bytes(),
        deserialize_byte_buf(),
        deserialize_option(),
        deserialize_unit(),
        deserialize_unit_struct(_name: &'static str),
        deserialize_seq(),
        deserialize_tuple(_len: usize),
        deserialize_tuple_struct(_name: &'static str, _len: usize),
        deserialize_map(),
        deserialize_enum(_name: &'static str, _variants: &'static [&'static str]),
        deserialize_identifier(),
        deserialize_ignored_any(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct SampleQuery {
        #[serde(rename = "MediaSourceId")]
        media_source_id: Option<String>,
        #[serde(rename = "UserId")]
        user_id: Option<String>,
        #[serde(rename = "Limit")]
        limit: Option<usize>,
        #[serde(rename = "IsPlayback")]
        is_playback: Option<bool>,
        #[serde(rename = "api_key")]
        api_key: Option<String>,
    }

    #[test]
    fn accepts_coexisting_token_aliases_without_duplicate_field_error() {
        let q: SampleQuery = parse_emby_query(
            "MediaSourceId=src_435094&Static=true&api_key=tok1&X-Emby-Token=tok2&ApiKey=tok3",
        )
        .unwrap();
        assert_eq!(q.media_source_id.as_deref(), Some("src_435094"));
        assert_eq!(q.api_key.as_deref(), Some("tok1"));
    }

    #[test]
    fn replaces_empty_token_with_later_non_empty_alias() {
        let q: SampleQuery = parse_emby_query("api_key=&X-Emby-Token=valid_tok").unwrap();
        assert_eq!(q.api_key.as_deref(), Some("valid_tok"));
    }

    #[test]
    fn matches_keys_case_insensitively_and_deduplicates() {
        let q: SampleQuery = parse_emby_query(
            "mediasourceid=src_1&MEDIASOURCEID=src_2&userid=u1&limit=20&isplayback=true",
        )
        .unwrap();
        assert_eq!(q.media_source_id.as_deref(), Some("src_1"));
        assert_eq!(q.user_id.as_deref(), Some("u1"));
        assert_eq!(q.limit, Some(20));
        assert_eq!(q.is_playback, Some(true));
    }

    #[test]
    fn rejects_invalid_typed_values_with_bad_request() {
        let err = parse_emby_query::<SampleQuery>("Limit=not_a_number").unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }
}
