use serde_json::{json, Value};

pub(in crate::web::emby_api) struct EpisodeJsonInput {
    pub(in crate::web::emby_api) server_id: String,
    pub(in crate::web::emby_api) work_id: i64,
    pub(in crate::web::emby_api) season_number: i32,
    pub(in crate::web::emby_api) episode_number: i32,
    pub(in crate::web::emby_api) title: Option<String>,
    pub(in crate::web::emby_api) series_name: Option<String>,
    pub(in crate::web::emby_api) overview: Option<String>,
    pub(in crate::web::emby_api) air_date: Option<String>,
    pub(in crate::web::emby_api) still_path: Option<String>,
    pub(in crate::web::emby_api) poster_path: Option<String>,
    pub(in crate::web::emby_api) playback_position_ticks: i64,
    pub(in crate::web::emby_api) duration_ticks: i64,
    pub(in crate::web::emby_api) runtime_seconds: i32,
    pub(in crate::web::emby_api) is_played: bool,
    pub(in crate::web::emby_api) media_sources: Option<Value>,
    pub(in crate::web::emby_api) series_primary_image_tag: bool,
}

pub(in crate::web::emby_api) fn build_episode_item_json(input: EpisodeJsonInput) -> Value {
    let EpisodeJsonInput {
        server_id,
        work_id,
        season_number,
        episode_number,
        title,
        series_name,
        overview,
        air_date,
        still_path,
        poster_path,
        playback_position_ticks,
        duration_ticks,
        runtime_seconds,
        is_played,
        media_sources,
        series_primary_image_tag,
    } = input;
    let image_path = still_path.as_deref().or(poster_path.as_deref());
    let mut item = json!({
        "Id": format!("ep_{work_id}_{season_number}_{episode_number}"),
        "Name": title.unwrap_or_else(|| format!("第 {} 集", episode_number)),
        "ServerId": server_id,
        "SeriesId": format!("work_{work_id}"),
        "SeasonId": format!("season_{work_id}_{season_number}"),
        "Type": "Episode",
        "IsFolder": false,
        "IndexNumber": episode_number,
        "ParentIndexNumber": season_number,
        "Overview": overview,
        "PremiereDate": air_date,
        "MediaType": "Video",
        "LocationType": "FileSystem",
        "ImageTags": if image_path.is_some() {
            json!({"Primary": crate::web::emby_api::images::image_path_tag(image_path)})
        } else {
            json!({})
        },
        "UserData": {
            "PlaybackPositionTicks": playback_position_ticks,
            "PlayCount": if is_played { 1 } else { 0 },
            "IsFavorite": false,
            "Played": is_played
        }
    });
    if let Some(series_name) = series_name {
        item["SeriesName"] = json!(series_name);
    }
    if series_primary_image_tag {
        item["SeriesPrimaryImageTag"] = json!(crate::web::emby_api::images::image_path_tag(
            poster_path.as_deref()
        ));
    }
    let effective_duration = if duration_ticks > 0 {
        duration_ticks
    } else {
        i64::from(runtime_seconds.max(0)) * 600_000_000
    };
    if effective_duration > 0 {
        item["RunTimeTicks"] = json!(effective_duration);
    }
    if let Some(media_sources) = media_sources {
        item["MediaSources"] = media_sources;
    }
    item
}
