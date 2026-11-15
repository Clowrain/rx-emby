use super::*;
use axum::{body::{Body, to_bytes}, http::Request};
use tower::ServiceExt;

async fn request(state: Arc<AppState>, method: &str, path: &str, payload: Value) -> (StatusCode,Value) {
    let req = Request::builder().method(method).uri(path)
        .header("Authorization","Bearer smart-test-admin-token")
        .header("Content-Type","application/json")
        .body(Body::from(payload.to_string())).unwrap();
    let response = crate::web::build_router(state).oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(),usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or_else(|_|json!(String::from_utf8_lossy(&bytes)));
    (status,value)
}

#[tokio::test]
#[ignore = "需要隔离 PostgreSQL：设置 TEST_DATABASE_URL 后运行 --ignored"]
async fn smart_view_end_to_end() {
    let url = std::env::var("TEST_DATABASE_URL").expect("使用隔离测试库");
    let db = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::migrate!("./migrations").run(&db).await.unwrap();
    sqlx::raw_sql("TRUNCATE users, media_works, sync_folders, virtual_libraries CASCADE").execute(&db).await.unwrap();
    sqlx::raw_sql(r#"
        INSERT INTO users(id,username,password_hash,is_admin,allowed_categories) VALUES
        ('admin','smart-admin','',TRUE,ARRAY['*']),('viewer','smart-viewer','',FALSE,ARRAY['vlib_*']),
        ('denied','smart-denied','',FALSE,ARRAY['电视剧']);
        INSERT INTO access_tokens(token,user_id,device_id,device_name,client_name) VALUES
        ('smart-test-admin-token','admin','test','test','test');
        INSERT INTO media_works(id,media_type,tmdb_id,title,rating,year,poster_path,category,countries,networks,genres,metadata_version,tmdb_metadata,premiered_at)
        VALUES
        (10,'tv',100,'低分剧',5,2020,'/low.jpg','电视剧',ARRAY['中国大陆'],ARRAY['Apple TV'],ARRAY['悬疑'],1,'{"original_language":"zh","vote_count":20,"status":"Returning Series","number_of_episodes":30,"episode_run_time":[45]}','2020-01-01'),
        (20,'tv',200,'高分剧',9,2026,'/high.jpg','电视剧',ARRAY['CN'],ARRAY['Apple TV'],ARRAY['悬疑','犯罪'],1,'{"original_language":"zh","vote_count":1500,"status":"Ended","number_of_episodes":8,"number_of_seasons":1,"episode_run_time":[40],"networks":[{"id":2552,"name":"Apple TV"}]}','2026-01-01'),
        (30,'movie',300,'动画电影',8.5,2025,'/movie.jpg','动漫',ARRAY['JP'],ARRAY[]::text[],ARRAY['动画'],1,'{"original_language":"ja","vote_count":2000,"runtime":88,"popularity":100,"collection":{"id":99,"name":"系列"},"production_companies":[{"id":42,"name":"制作公司"}],"genre_ids":[16]}','2025-01-01');
        INSERT INTO sync_folders(id,folder_path,parsed_title,media_type,category,scrape_state,work_id)
        VALUES
        (10,'/low','低分剧','tv','电视剧','verified',10),
        (20,'/high','高分剧','tv','电视剧','verified',20),
        (30,'/movie','动画电影','movie','动漫','verified',30),
        (40,'/raw','未刮削','movie','电影','pending',NULL);
        INSERT INTO sync_files(folder_id,version_tag,alist_path,path_hash,file_name,container) VALUES
          (30,'4K · 杜比视界','/test/1',1,'1.mkv','mkv'),
          (20,'4K','/test/2',2,'2.mkv','mkv'),(20,'1080p · 杜比视界','/test/3',3,'3.mkv','mkv');
    "#).execute(&db).await.unwrap();
    let (tx,_rx)=tokio::sync::mpsc::channel(16);
    let state=AppState::new(crate::config::BootstrapFileConfig::default().into_dynamic(),db.clone(),tx).unwrap();
    let payload=json!({"name":"仅剧集","icon_type":"tvshows","filter_rules":{"media_type":"tv"},"is_enabled":true});
    assert_eq!(request(state.clone(),"POST","/api/admin/virtual_libraries",payload).await.0,StatusCode::OK);
    let vid:i32=sqlx::query_scalar("SELECT id FROM virtual_libraries WHERE name='仅剧集'").fetch_one(&db).await.unwrap();
    let pid=format!("lib_vlib_{}",vid);
    let views=get_user_views(State(state.clone()),HeaderMap::new(),Path("viewer".into())).await;
    assert!(views.0["Items"].as_array().unwrap().iter().any(|v|v["Id"]==pid));
    let items=get_user_items(State(state.clone()),HeaderMap::new(),Path("viewer".into()),Query(ItemsQuery{parent_id:Some(pid.clone()),..Default::default()})).await.unwrap();
    assert_eq!(items.0["TotalRecordCount"],2);
    let conflict=get_user_items(State(state.clone()),HeaderMap::new(),Path("viewer".into()),Query(ItemsQuery{parent_id:Some(pid.clone()),include_item_types:Some("Movie".into()),..Default::default()})).await.unwrap();
    assert_eq!(conflict.0["TotalRecordCount"],0);
    let denied=get_user_items(State(state.clone()),HeaderMap::new(),Path("denied".into()),Query(ItemsQuery{parent_id:Some(pid.clone()),..Default::default()})).await.unwrap_err();
    assert_eq!(denied.0,StatusCode::FORBIDDEN);
    // 管理计数、客户端规则和封面使用同一谓词，选图按全局评分排序。
    let rule=VirtualLibraryRule{media_type:Some("tv".into()),..Default::default()};
    assert_eq!(count_virtual_library_matches(&state,&rule).await.unwrap(),2);
    assert_eq!(fetch_virtual_library_top_media_images(&state,&rule,1).await[0].1.as_deref(),Some("/high.jpg"));
    let cases = vec![
      (json!({"countries":["CN"]}),2), (json!({"countries":["中国大陆"]}),2),
      (json!({"networks":["Apple TV+"]}),2), (json!({"network_ids":[2552]}),1),
      (json!({"min_rating":8,"min_votes":1000}),2),
      (json!({"languages":["ja"],"runtime_max":90,"company_ids":[42],"collection_ids":[99],"genre_ids":[16]}),1),
      (json!({"statuses":["Ended"],"episodes_max":12,"seasons_max":1}),1),
      (json!({"runtime_min":39,"runtime_max":41,"media_type":"tv"}),1),
      (json!({"has_collection":false}),0), (json!({"has_collection":true}),1),
      (json!({"version_tags":["4K","杜比视界"]}),1),
      (json!({"genres":["悬疑","犯罪"],"genres_all":true}),1),
      (json!({"countries":["CN"],"exclude_genres":["犯罪"]}),1),
      (json!({"any":[{"countries":["CN"],"min_rating":8},{"languages":["ja"]}]}),2),
      (json!({"min_votes":0}),3), (json!({"runtime_max":1000}),3),
      (json!({"keyword":"%"}),0), (json!({"premiered_after":"2026-01-01"}),1),
      (json!({"min_popularity":50}),1), (json!({"added_within_days":1}),4),
    ];
    for (rule,expected) in cases {
        let (status,result)=request(state.clone(),"POST","/api/admin/virtual_libraries/preview",rule.clone()).await;
        assert_eq!(status,StatusCode::OK,"{rule}: {result}");
        assert_eq!(result["matched_count"],expected,"{rule}");
    }
    for bad in [json!({"min_rating":11}),json!({"year_min":2026,"year_max":2020}),json!({"runtime_min":-1})] {
        let (status,_)=request(state.clone(),"POST","/api/admin/virtual_libraries",json!({"name":"无效","icon_type":"movies","filter_rules":bad})).await;
        assert_eq!(status,StatusCode::BAD_REQUEST);
    }
    assert_eq!(request(state.clone(),"POST","/api/admin/virtual_libraries",json!({"id":2147483647,"name":"不存在","icon_type":"movies","filter_rules":{}})).await.0,StatusCode::NOT_FOUND);
    let (_,options)=request(state.clone(),"GET","/api/admin/virtual_libraries/options",Value::Null).await;
    assert_eq!(options["companies"][0]["id"],42);
    assert_eq!(options["metadata"]["enriched"],3);
    // 两个同时到来的预设请求仅创建一份。
    let (a,b)=tokio::join!(request(state.clone(),"POST","/api/admin/virtual_libraries/presets",Value::Null),request(state.clone(),"POST","/api/admin/virtual_libraries/presets",Value::Null));
    assert_eq!(a.0,StatusCode::OK); assert_eq!(b.0,StatusCode::OK);
    assert_eq!(a.1["inserted"].as_i64().unwrap()+b.1["inserted"].as_i64().unwrap(),9);
    verify_metadata_refresh(&db).await;
    // 接近当前库规模的回归数据，检查参数化条件仍走普通 SQL 过滤。
    let mut transaction=db.begin().await.unwrap();
    sqlx::raw_sql("INSERT INTO media_works(id,media_type,title,category,rating,countries) SELECT 10000+g,'movie','规模测试','电影',8,ARRAY['CN'] FROM generate_series(1,70000) g")
        .execute(&mut *transaction).await.unwrap();
    let rule=VirtualLibraryRule{countries:vec!["CN".into()],min_rating:Some(8.0),..Default::default()};
    let sql=format!("SELECT COUNT(*) FROM media_works m WHERE {}",rule.sql_predicate("$1::jsonb"));
    let started=std::time::Instant::now();
    let count:i64=sqlx::query_scalar(&sql).bind(serde_json::to_value(rule).unwrap()).fetch_one(&mut *transaction).await.unwrap();
    assert!(count>=70000);
    eprintln!("70,000 条规模测试计数耗时：{:?}",started.elapsed());
    transaction.rollback().await.unwrap();
    // 规则损坏明确报错，不放宽为全库。
    sqlx::query("UPDATE virtual_libraries SET filter_rules='{\"min_rating\":\"bad\"}' WHERE id=$1").bind(vid).execute(&db).await.unwrap();
    assert_eq!(get_user_items(State(state.clone()),HeaderMap::new(),Path("viewer".into()),Query(ItemsQuery{parent_id:Some(pid),..Default::default()})).await.unwrap_err().0,StatusCode::INTERNAL_SERVER_ERROR);
}


async fn verify_metadata_refresh(db: &sqlx::PgPool) {
    use std::sync::atomic::{AtomicUsize,Ordering};
    let calls=Arc::new(AtomicUsize::new(0));
    let seen=calls.clone();
    let mock=Router::new().route("/movie/500",get(move || {
        let seen=seen.clone(); async move {
            seen.fetch_add(1,Ordering::SeqCst);
            Json(json!({"id":500,"title":"刷新后的电影","vote_average":8.8,"original_language":"ja","vote_count":1000,"runtime":89,
                "production_countries":[{"iso_3166_1":"JP"}],"genres":[{"id":16,"name":"动画"}],"production_companies":[{"id":42,"name":"制作公司"}],
                "belongs_to_collection":{"id":99,"name":"电影系列"}}))
        }
    })).route("/tv/600",get(||async{Json(json!({"id":600,"name":"剧集","status":"Ended","number_of_episodes":3,"origin_country":["CN"],"networks":[{"id":2552,"name":"Apple TV"}]}))}))
      .route("/tv/600/season/1",get(||async{Json(json!({"name":"第一季","episodes":[
          {"episode_number":1,"name":"第一集","overview":"简介","still_path":"/1.jpg","runtime":40},
          {"episode_number":2,"name":"第二集","overview":"简介","still_path":"/2.jpg","runtime":41},
          {"episode_number":3,"name":"第三集","overview":"简介","still_path":"/3.jpg","runtime":42}
      ]}))}));
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr=listener.local_addr().unwrap();
    let server=tokio::spawn(async move{ axum::serve(listener,mock).await.unwrap() });
    let mut cfg=crate::config::BootstrapFileConfig::default().into_dynamic();
    cfg.tmdb_api_base=format!("http://{}",addr);
    cfg.tmdb_api_key="fixture".into(); cfg.scraper_auto_background=false;
    let (tx,_rx)=tokio::sync::mpsc::channel(16);
    let state=AppState::new(cfg,db.clone(),tx).unwrap();
    sqlx::raw_sql(r#"
      INSERT INTO media_works(id,media_type,tmdb_id,title,category,metadata_refresh_requested,is_locked) VALUES
        (500,'movie',500,'旧名称','电影',TRUE,FALSE),
        (502,'movie',5000,'锁定名称','电影',FALSE,TRUE),
        (600,'tv',600,'剧集','电视剧',FALSE,FALSE),
        (700,'movie',700,'失败保留','电影',TRUE,FALSE);
      INSERT INTO sync_folders(id,folder_path,parsed_title,hint_tmdb_id,media_type,category,scrape_state,work_id) VALUES
        (500,'/fixture/500','刷新后的电影',500,'movie','电影','pending',500),
        (501,'/fixture/501','刷新后的电影',500,'movie','电影','pending',NULL),
        (600,'/fixture/600','剧集',600,'tv','电视剧','verified',600),
        (700,'/fixture/700','失败保留',700,'movie','电影','pending',700);
      INSERT INTO work_seasons(work_id,season_number,name) VALUES(600,1,'第一季');
      INSERT INTO sync_files(folder_id,season_number,episode_number,version_tag,alist_path,path_hash,file_name,container) VALUES
        (600,1,1,'默认版本','/fixture/600/1',6001,'1.mkv','mkv');
    "#).execute(db).await.unwrap();
    crate::scraper::scrape_single_folder(&state,500).await.unwrap();
    crate::scraper::scrape_single_folder(&state,501).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst),1,"同作品多个目录共用一次详情请求并合并至同一 work");
    let work_id_501:Option<i64>=sqlx::query_scalar("SELECT work_id FROM sync_folders WHERE id=501").fetch_one(db).await.unwrap();
    assert_eq!(work_id_501,Some(500));
    crate::scraper::scrape_work_seasons_by_id(&state,600,true).await.unwrap();
    let season:(i32,i32)=sqlx::query_as("SELECT episode_count,scrape_attempts FROM work_seasons WHERE work_id=600 AND season_number=1").fetch_one(db).await.unwrap();
    assert_eq!(season,(3,1),"真实集数与刮削次数分开保存");
    assert!(crate::scraper::scrape_single_folder(&state,700).await.is_err());
    let retry_cnt:i32=sqlx::query_scalar("SELECT retry_count FROM sync_folders WHERE id=700").fetch_one(db).await.unwrap();
    assert_eq!(retry_cnt,1);
    server.abort();
}
