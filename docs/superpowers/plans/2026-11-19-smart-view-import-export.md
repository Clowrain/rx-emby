# Smart View Configuration Import, Export & AI Template Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove hardcoded smart view presets and replace them with a transactional `.zip` / `.json` configuration import, preview, and export system that bundles custom static covers and an auto-generated `AI-GUIDE.md` reference template.

**Architecture:** Extract archive parsing, image validation/normalization, and `AI-GUIDE.md` generation into a dedicated Rust module (`src/web/admin_api/smart_view_bundle.rs`) shared by the preview and apply endpoints. Store normalized custom static covers (`custom_<16hex>.jpg`) under `{image_cache_dir}/custom_covers/` outside the LRU cache table, serve them to both the admin UI (`/covers/*path`) and Emby clients (`/Items/lib_vlib_{id}/Images/Primary`), and expose a two-step preview-and-confirm modal in Vue 3.

**Tech Stack:** Rust (Axum 0.7, SQLx 0.8, `zip` 2.2, `image` 0.25, `xxhash-rust`), Vue 3 (`<script setup>`, Tailwind CSS), Node.js built-in test runner (`node --test`).

**Spec:** [`docs/superpowers/specs/2026-11-19-smart-view-import-export-design.md`](file:///Users/oneal/data/dev/projects/nas/rx-emby/docs/superpowers/specs/2026-11-19-smart-view-import-export-design.md)

## Global Constraints

- Remove `POST /api/admin/virtual_libraries/presets` and the frontend「补充 9 个预设视图」button completely.
- Top action bar buttons must use fixed labels regardless of whether the list is empty:「下载配置与 AI 模板」and「导入视图配置」.
- Import modal must include a persistent shortcut link:「下载当前配置与 AI 编写模板 (.zip)」.
- Import accepts both `.zip` archives and standalone `.json` files (both `{ "version": 1, "views": [...] }` object wrapper and top-level `[...]` array); `.rar` files must be rejected with `暂不支持 .rar 格式，请上传 .zip 压缩包或 .json 配置文件`.
- All-or-nothing validation: any invalid view name, duplicate view name within the bundle, invalid `cover_image`, or invalid `VirtualLibraryRule` must reject the entire bundle with `第 {idx} 个视图「{name}」配置有误：{reason}`.
- Same-name views must be updated in place (preserving `virtual_libraries.id` and `users.allowed_categories` grants) and increment `cover_revision`.
- Custom static covers in `.zip` (`covers/<file>`) must be decoded via `image` crate (JPEG/PNG/WebP, max 4 MiB raw, max 4096×4096 px), resized proportionally to max width 1280 px, encoded as JPEG, named `custom_{xxh3_64_hex:016x}`, and saved under `{image_cache_dir}/custom_covers/custom_{16hex}.jpg` (never added to `image_cache_index`).

## Review Focus

1. **`.rar` or non-zip/non-json binary upload:** Uploading a file starting with `Rar!\x1A\x07` or random bytes returns HTTP 400 with `暂不支持 .rar 格式，请上传 .zip 压缩包或 .json 配置文件` (for RAR) or a clear format error without panicking.
2. **Duplicate view names within the same import bundle:** Uploading a `.json` or `.zip` where two views have the same trimmed name (e.g., `"高分电影"` and `" 高分电影 "`) is rejected before any DB mutation with `导入包内存在重复的视图名称：「高分电影」，请保持名称唯一`.
3. **Standalone `.json` referencing a missing cover file vs existing `custom_<16hex>`:** A `.json` file referencing `"covers/not_uploaded.png"` fails validation telling the user to upload a `.zip` containing the image, whereas referencing `"custom_<16hex>"` (or `"covers/custom_<16hex>.jpg"`) that already exists in `{image_cache_dir}/custom_covers/` succeeds.
4. **AI-generated shorthand fields (`icon_type: "movie" | "tv"`, missing `sort_order`, top-level JSON array):** Shorthand values normalize cleanly (`"movie"` → `"movies"`, `"tv"` → `"tvshows"`, `sort_order` defaults to `(idx + 1) * 10`) instead of failing validation.
5. **Path traversal or oversized entry in `.zip`:** A `.zip` referencing `"covers/../../etc/passwd"` or an invalid image byte stream is rejected with a view-specific error and never reads or writes outside `{image_cache_dir}/custom_covers/`.

---

### Task 1: Bundle Parser, Custom Cover Normalizer & AI Guide Generator

**Files:**
- Create: `src/web/admin_api/smart_view_bundle.rs`
- Modify: `src/web/admin_api/mod.rs:11-16`
- Modify: `src/web/emby_api/images.rs:534-550` (make `is_builtin_cover_key` `pub(crate)`)

**Interfaces:**
- Consumes: `crate::smart_views::VirtualLibraryRule`, `crate::web::emby_api::images::is_builtin_cover_key`
- Produces (in `crate::web::admin_api::smart_view_bundle`):
  - `pub struct ExportableView { pub name: String, pub icon_type: String, pub sort_order: i32, pub is_enabled: bool, pub cover_image: Option<String>, pub filter_rules: VirtualLibraryRule }`
  - `pub struct ValidatedImportView { pub name: String, pub icon_type: String, pub sort_order: i32, pub is_enabled: bool, pub cover_image: Option<String>, pub cover_type: &'static str, pub cover_label: String, pub filter_rules: VirtualLibraryRule }`
  - `pub struct ParsedImportBundle { pub views: Vec<ValidatedImportView>, pub custom_covers: HashMap<String, Vec<u8>> }`
  - `pub fn is_custom_cover_key(key: &str) -> bool`
  - `pub fn parse_and_validate_import_bundle(bytes: &[u8], custom_covers_dir: &std::path::Path) -> Result<ParsedImportBundle, String>`
  - `pub fn build_smart_view_export_zip(views: &[ExportableView], facets: &serde_json::Value, custom_covers_dir: &std::path::Path) -> Result<Vec<u8>, String>`

- [ ] **Step 1: Write the failing unit tests in `src/web/admin_api/smart_view_bundle.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_rar_magic_bytes_with_clear_copy() {
        let dir = std::env::temp_dir().join(format!("rx_test_{}", uuid::Uuid::new_v4().simple()));
        let rar_bytes = b"Rar!\x1A\x07\x00dummy";
        let err = parse_and_validate_import_bundle(rar_bytes, &dir).unwrap_err();
        assert_eq!(err, "暂不支持 .rar 格式，请上传 .zip 压缩包或 .json 配置文件");
    }

    #[test]
    fn normalizes_ai_shorthand_array_and_rejects_duplicate_names() {
        let dir = std::env::temp_dir().join(format!("rx_test_{}", uuid::Uuid::new_v4().simple()));
        let raw = br#"[
            {"name": " 万人高分电影榜 ", "icon_type": "movie", "cover_image": "collage_cards", "filter_rules": {"media_type": "movie", "min_rating": 8.0, "min_votes": 1000}},
            {"name": "已完结短剧", "icon_type": "tv", "cover_image": "auto", "filter_rules": {"media_type": "tv", "statuses": ["Ended"], "episodes_max": 12}}
        ]"#;
        let parsed = parse_and_validate_import_bundle(raw, &dir).unwrap();
        assert_eq!(parsed.views.len(), 2);
        assert_eq!(parsed.views[0].name, "万人高分电影榜");
        assert_eq!(parsed.views[0].icon_type, "movies");
        assert_eq!(parsed.views[0].sort_order, 10);
        assert_eq!(parsed.views[0].cover_type, "dynamic");
        assert_eq!(parsed.views[1].icon_type, "tvshows");
        assert_eq!(parsed.views[1].sort_order, 20);

        let dup = br#"[{"name": "高分"}, {"name": " 高分 "}]"#;
        let err = parse_and_validate_import_bundle(dup, &dir).unwrap_err();
        assert_eq!(err, "导入包内存在重复的视图名称：「高分」，请保持名称唯一");
    }

    #[test]
    fn rejects_unknown_cover_or_missing_zip_image_and_path_traversal() {
        let dir = std::env::temp_dir().join(format!("rx_test_{}", uuid::Uuid::new_v4().simple()));
        let bad_cover = br#"[{"name": "测试", "cover_image": "covers/missing.png"}]"#;
        let err = parse_and_validate_import_bundle(bad_cover, &dir).unwrap_err();
        assert!(err.contains("第 1 个视图「测试」配置有误"));

        let traversal = br#"[{"name": "越权", "cover_image": "covers/../../etc/passwd"}]"#;
        let err2 = parse_and_validate_import_bundle(traversal, &dir).unwrap_err();
        assert!(err2.contains("第 1 个视图「越权」配置有误"));
    }

    #[test]
    fn zip_roundtrip_bundles_custom_cover_and_ai_guide() {
        let dir = std::env::temp_dir().join(format!("rx_test_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut img = image::RgbImage::new(32, 18);
        for p in img.pixels_mut() {
            *p = image::Rgb([20, 180, 120]);
        }
        let mut png_bytes = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut png_bytes), image::ImageFormat::Png)
            .unwrap();

        let mut zip_buf = std::io::Cursor::new(Vec::new());
        {
            let mut zw = zip::ZipWriter::new(&mut zip_buf);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("views.json", opts).unwrap();
            std::io::Write::write_all(
                &mut zw,
                br#"{"version":1,"views":[{"name":"自定义封面视图","icon_type":"movies","cover_image":"covers/my_banner.png","filter_rules":{"media_type":"movie"}}]}"#,
            ).unwrap();
            zw.start_file("covers/my_banner.png", opts).unwrap();
            std::io::Write::write_all(&mut zw, &png_bytes).unwrap();
            zw.finish().unwrap();
        }
        let parsed = parse_and_validate_import_bundle(&zip_buf.into_inner(), &dir).unwrap();
        assert_eq!(parsed.views.len(), 1);
        let assigned_key = parsed.views[0].cover_image.clone().unwrap();
        assert!(is_custom_cover_key(&assigned_key));
        assert_eq!(parsed.views[0].cover_type, "custom");
        let jpg_bytes = parsed.custom_covers.get(&assigned_key).unwrap();
        std::fs::write(dir.join(format!("{assigned_key}.jpg")), jpg_bytes).unwrap();

        let exported_zip = build_smart_view_export_zip(
            &[ExportableView {
                name: "自定义封面视图".into(),
                icon_type: "movies".into(),
                sort_order: 10,
                is_enabled: true,
                cover_image: Some(assigned_key.clone()),
                filter_rules: VirtualLibraryRule::default(),
            }],
            &json!({"companies": [{"id": 42, "name": "吉卜力"}]}),
            &dir,
        )
        .unwrap();

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&exported_zip)).unwrap();
        assert!(archive.by_name("AI-GUIDE.md").is_ok());
        assert!(archive.by_name(&format!("covers/{assigned_key}.jpg")).is_ok());
        let re_imported = parse_and_validate_import_bundle(&exported_zip, &dir).unwrap();
        assert_eq!(re_imported.views[0].cover_image.as_deref(), Some(assigned_key.as_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 2: Run `cargo test smart_view_bundle` to verify it fails**

Run: `cargo test smart_view_bundle`
Expected: FAIL (module not yet implemented)

- [ ] **Step 3: Implement `src/web/admin_api/smart_view_bundle.rs` and expose `pub(crate) fn is_builtin_cover_key` in `src/web/emby_api/images.rs`**

Implement:
- `is_custom_cover_key(key: &str) -> bool`: matches `key.strip_prefix("custom_").is_some_and(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))`.
- `parse_and_validate_import_bundle(bytes: &[u8], custom_covers_dir: &Path) -> Result<ParsedImportBundle, String>`:
  - Reject empty input or `bytes.starts_with(b"Rar!\x1A\x07")`.
  - If `bytes.starts_with(b"PK\x03\x04")`, open `zip::ZipArchive`, read `views.json` (or single root `.json` file) with `Read::take(1024 * 1024 + 1)`, and load any referenced `covers/<basename>` entries with `Read::take(4 * 1024 * 1024 + 1)`.
  - Otherwise parse `bytes` directly as UTF-8 JSON (capped at 1 MiB).
  - Support both `{"views": [...]}` and `[...]` JSON structures; enforce `1..=100` views, unique trimmed names (`1..=64` chars), normalized `icon_type` (`"movies"` / `"tvshows"`), and `rule.validate()`.
  - Decode custom images via `image::load_from_memory`, check `width <= 4096 && height <= 4096`, resize if `width > 1280` via `FilterType::Lanczos3`, encode as JPEG quality 88, hash with `xxh3_64`, and store under key `custom_{hash:016x}`.
- `build_smart_view_export_zip(views: &[ExportableView], facets: &Value, custom_covers_dir: &Path) -> Result<Vec<u8>, String>`:
  - Write `views.json` (rewriting any `custom_<16hex>` cover to `"covers/custom_<16hex>.jpg"` if the file exists on disk), `AI-GUIDE.md` (with the 8 TMDB field groups, full `VirtualLibraryRule` reference, cover options, 6-category 18 reference view templates, and live `facets` ID tables), and `covers/custom_<16hex>.jpg`.

- [ ] **Step 4: Run `cargo test smart_view_bundle` to verify all unit tests pass**

Run: `cargo test smart_view_bundle`
Expected: PASS (4 tests passing)

---

### Task 2: Backend Export/Import Endpoints & Custom Cover Serving

**Files:**
- Modify: `src/web/admin_api/mod.rs:146-179`
- Modify: `src/web/admin_api/virtual_libraries.rs`
- Modify: `src/web/mod.rs:24-35,70-74`
- Modify: `src/web/emby_api/images.rs:628-680`
- Modify: `src/web/smart_view_tests.rs:320-342`

**Interfaces:**
- Consumes: `crate::web::admin_api::smart_view_bundle::*`
- Produces:
  - `GET /api/admin/virtual_libraries/export` → `application/zip` attachment (`rx-emby-smart-views.zip`)
  - `POST /api/admin/virtual_libraries/import/preview` → JSON `{ total_count, create_count, update_count, custom_cover_count, unmatched_existing_count, items }`
  - `POST /api/admin/virtual_libraries/import/apply?prune_missing=bool` → JSON `{ status: "ok", created, updated, deleted, message }`
  - Custom cover serving on `GET /covers/custom_<16hex>.jpg` and `GET /Items/lib_vlib_{id}/Images/Primary`

- [ ] **Step 1: Write unit & integration tests for custom cover serving and import preview/apply in `src/web/admin_api/virtual_libraries.rs` and `src/web/smart_view_tests.rs`**

Add tests in `src/web/admin_api/virtual_libraries.rs` and replace the old `/api/admin/virtual_libraries/presets` test block in `src/web/smart_view_tests.rs` with export + preview + apply + prune verification.

- [ ] **Step 2: Implement routes and handlers in `src/web/admin_api/virtual_libraries.rs`, `src/web/admin_api/mod.rs`, `src/web/mod.rs`, and `src/web/emby_api/images.rs`**

- Remove `seed_preset_virtual_libraries` and `/api/admin/virtual_libraries/presets`.
- Add `export_virtual_libraries`, `preview_import_virtual_libraries`, and `apply_import_virtual_libraries` (with `axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024)`).
- In `save_virtual_library`, also allow `is_custom_cover_key` when saving an existing view that already uses a custom cover, and validate `cover_image` against built-in, dynamic (`auto` / `collage_*`), or existing custom cover keys.
- In `src/web/mod.rs` (`serve_covers`), check if `path` matches `custom_<16hex>.jpg` and serve from `{state.config.load().image_cache_dir}/custom_covers/{file}` before falling back to `WebAssets`.
- In `src/web/emby_api/images.rs` (`serve_library_cover`), if `custom_cover` satisfies `is_custom_cover_key(c)`, read `{image_cache_dir}/custom_covers/{c}.jpg` and return `image/jpeg` before falling back to embedded covers.
- Add orphan custom cover cleanup helper `cleanup_orphan_custom_covers(&state.db, &custom_covers_dir)` called after `delete_virtual_library`, `save_virtual_library`, and `apply_import_virtual_libraries`.

- [ ] **Step 3: Run `cargo test` to verify compilation and all unit tests pass**

Run: `cargo test`
Expected: PASS with 0 warnings

---

### Task 3: Frontend Import Preview Modal, Copy Update & Tests

**Files:**
- Modify: `frontend/src/api/admin.js:237-272`
- Modify: `frontend/src/utils/smartViewLabels.js:31-47`
- Modify: `frontend/src/composables/useVirtualLibraries.js`
- Create: `frontend/src/components/virtual-libraries/VirtualLibraryImportModal.vue`
- Modify: `frontend/src/components/VirtualLibrariesTab.vue`
- Modify: `frontend/src/components/virtual-libraries/VirtualLibraryFormModal.vue`
- Modify: `frontend/tests/smart-views.test.mjs`

**Interfaces:**
- Consumes:
  - `GET /api/admin/virtual_libraries/export`
  - `POST /api/admin/virtual_libraries/import/preview`
  - `POST /api/admin/virtual_libraries/import/apply?prune_missing=true|false`
- Produces:
  - `adminApi.exportVirtualLibrariesBundle()`
  - `adminApi.previewImportVirtualLibraries(bytes)`
  - `adminApi.applyImportVirtualLibraries(bytes, pruneMissing)`
  - `isCustomCover(val)` in `frontend/src/utils/smartViewLabels.js`
  - `VirtualLibraryImportModal.vue` component

- [ ] **Step 1: Add failing tests in `frontend/tests/smart-views.test.mjs`**

Add tests asserting:
- `VirtualLibrariesTab.vue` no longer contains `补充 9 个预设视图` or `handleSeedPresets`, and renders the fixed buttons `下载配置与 AI 模板` and `导入视图配置`.
- `useVirtualLibraries` import workflow: selecting a file calls `previewImportVirtualLibraries`, populates `importModal.preview`, and confirming calls `applyImportVirtualLibraries(rawBytes, importModal.pruneMissing)` and reloads libraries.
- `isCustomCover('custom_0123456789abcdef')` returns `true` and `coverImageUrl` resolves to `/covers/custom_0123456789abcdef.jpg?_t=...`.

- [ ] **Step 2: Run `node --test frontend/tests/smart-views.test.mjs` to verify it fails**

Run: `node --test frontend/tests/smart-views.test.mjs`
Expected: FAIL

- [ ] **Step 3: Implement frontend API, composable state, `VirtualLibraryImportModal.vue`, and `VirtualLibrariesTab.vue` updates**

- Update `frontend/src/api/admin.js`: remove `seedPresetVirtualLibraries`, add `exportVirtualLibrariesBundle` (fetches `/api/admin/virtual_libraries/export` with `Authorization: Bearer ...` and triggers browser download `rx-emby-smart-views.zip`), `previewImportVirtualLibraries(bytes)`, and `applyImportVirtualLibraries(bytes, pruneMissing)`.
- Update `frontend/src/utils/smartViewLabels.js`: export `isCustomCover(val)`.
- Update `frontend/src/composables/useVirtualLibraries.js`: remove `seeding` and `handleSeedPresets`; add `exporting`, `importModal` reactive state (`visible`, `fileName`, `rawBytes`, `loadingPreview`, `applying`, `pruneMissing`, `error`, `preview`), `handleDownloadBundle()`, `openImportModal()`, `handleImportFileSelected(file)`, `confirmImportApply()`.
- Create `frontend/src/components/virtual-libraries/VirtualLibraryImportModal.vue` with file drop/select zone, persistent「下载当前配置与 AI 编写模板 (.zip)」shortcut button, parsed view list with `新增` / `同名更新` badges, cover type badges, `ruleBadges` summary, conditional `同时移除本次配置中未包含的 N 个旧视图` checkbox, and `确认应用配置（N 项）` primary CTA.
- Update `frontend/src/components/VirtualLibrariesTab.vue` and `frontend/src/components/virtual-libraries/VirtualLibraryFormModal.vue` (if a view currently has a custom cover `isCustomCover(modal.coverImage)`, display a current custom cover indicator button in the cover picker so editing the view doesn't accidentally wipe its custom cover).

- [ ] **Step 4: Run frontend tests and build to verify everything passes**

Run: `node --test frontend/tests/*.test.mjs && npm --prefix frontend run build`
Expected: PASS and clean Vite build into `web_ui/`

- [ ] **Step 5: Run full backend test suite (`cargo test`) after frontend build embeds `web_ui/`**

Run: `cargo test`
Expected: PASS
