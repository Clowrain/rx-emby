use super::auth::resolve_valid_user_id_with_query;
use super::query::EmbyQuery as Query;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Deserialize, Default)]
pub(super) struct DisplayPrefsQuery {
    #[serde(rename = "UserId", alias = "userId")]
    user_id: Option<String>,
    #[serde(rename = "Client", alias = "client")]
    client: Option<String>,
    #[serde(
        rename = "api_key",
        alias = "ApiKey",
        alias = "X-Emby-Token",
        alias = "x-emby-token"
    )]
    api_key: Option<String>,
}

fn valid_post_payload(prefs_id: &str, payload: &Value) -> bool {
    !prefs_id.is_empty() && prefs_id.chars().count() <= 128 && payload.is_object()
}

fn valid_client(client: Option<&str>) -> bool {
    client.is_none_or(|value| value.chars().count() <= 64)
}

fn valid_get_input(prefs_id: &str, client: Option<&str>) -> bool {
    !prefs_id.is_empty() && prefs_id.chars().count() <= 128 && valid_client(client)
}

fn default_display_prefs(prefs_id: &str, client: &str) -> Value {
    json!({
        "Id": prefs_id, "ViewType": null, "SortBy": "SortName", "IndexBy": null,
        "RememberIndexing": false, "PrimaryImageHeight": 250, "PrimaryImageWidth": 250,
        "CustomPrefs": {}, "ScrollDirection": "Horizontal", "ShowBackdrop": true,
        "RememberSorting": false, "SortOrder": "Ascending", "Client": client
    })
}

/// `GET /DisplayPreferences/:display_preferences_id`
pub(super) async fn get_display_preferences(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(prefs_id): Path<String>,
    Query(q): Query<DisplayPrefsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    let client = q
        .client
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("emby");
    if !valid_get_input(&prefs_id, Some(client)) {
        return Err((StatusCode::BAD_REQUEST, "显示偏好参数无效".into()));
    }

    let saved: Option<Value> = sqlx::query_scalar(
        r#"
        SELECT prefs_json
        FROM user_display_preferences
        WHERE user_id = $1 AND client = $2 AND display_prefs_id = $3
        "#,
    )
    .bind(&uid)
    .bind(client)
    .bind(&prefs_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(mut prefs) = saved {
        if !prefs.is_object() {
            prefs = json!({});
        }
        let defaults = default_display_prefs(&prefs_id, client);
        if let (Some(object), Some(default_object)) = (prefs.as_object_mut(), defaults.as_object())
        {
            for (key, value) in default_object {
                object.entry(key.clone()).or_insert_with(|| value.clone());
            }
        }
        return Ok(Json(prefs));
    }

    Ok(Json(default_display_prefs(&prefs_id, client)))
}

/// `POST /DisplayPreferences/:display_preferences_id`
pub(super) async fn post_display_preferences(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(prefs_id): Path<String>,
    Query(q): Query<DisplayPrefsQuery>,
    Json(payload): Json<Value>,
) -> Result<StatusCode, (StatusCode, String)> {
    if !valid_post_payload(&prefs_id, &payload) {
        return Err((StatusCode::BAD_REQUEST, "无效的显示偏好设置".into()));
    }
    let uid = resolve_valid_user_id_with_query(
        &state,
        q.user_id.as_deref(),
        &headers,
        q.api_key.as_deref(),
    )
    .await?;
    let client = q
        .client
        .as_deref()
        .or_else(|| payload.get("Client").and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("emby");
    if !valid_client(Some(client)) {
        return Err((StatusCode::BAD_REQUEST, "Client 长度无效".into()));
    }

    sqlx::query(
        r#"
        INSERT INTO user_display_preferences (user_id, client, display_prefs_id, prefs_json, updated_at)
        VALUES ($1, $2, $3, $4, NOW())
        ON CONFLICT (user_id, client, display_prefs_id)
        DO UPDATE SET prefs_json = EXCLUDED.prefs_json, updated_at = NOW()
        "#,
    )
    .bind(&uid)
    .bind(client)
    .bind(&prefs_id)
    .bind(&payload)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_validation_rejects_empty_or_oversized_id_and_non_object() {
        assert!(!valid_post_payload("", &json!({})));
        assert!(!valid_post_payload(&"x".repeat(129), &json!({})));
        assert!(!valid_post_payload("home", &Value::Null));
        assert!(valid_post_payload("home", &json!({"SortBy":"SortName"})));
        assert!(!valid_client(Some(&"x".repeat(65))));
        assert!(valid_client(Some(&"x".repeat(64))));
        assert!(!valid_get_input("", Some("emby")));
        assert!(!valid_get_input("home", Some(&"x".repeat(65))));
        assert!(valid_get_input("home", Some("emby")));
    }

    #[test]
    fn get_defaults_are_normalized_for_existing_preferences() {
        let mut prefs = json!({"SortBy":"DateCreated"});
        let defaults = json!({
            "Id":"home", "ViewType":null, "SortBy":"SortName", "IndexBy":null,
            "RememberIndexing":false, "PrimaryImageHeight":250, "PrimaryImageWidth":250,
            "CustomPrefs":{}, "ScrollDirection":"Horizontal", "ShowBackdrop":true,
            "RememberSorting":false, "SortOrder":"Ascending", "Client":"emby"
        });
        for (key, value) in defaults.as_object().unwrap() {
            prefs
                .as_object_mut()
                .unwrap()
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        assert_eq!(prefs["SortBy"], "DateCreated");
        assert_eq!(prefs["PrimaryImageWidth"], 250);
        assert_eq!(prefs["Client"], "emby");
    }
}
