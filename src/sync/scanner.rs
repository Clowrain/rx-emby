use super::parser::{extract_clean_alist_path, ParsedStrmRecord};
use std::collections::HashMap;

#[derive(Default)]
pub struct SubtitleIndex {
    folders: HashMap<String, Vec<String>>,
}
impl SubtitleIndex {
    pub fn add_line(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return;
        }
        let raw = line
            .split_once('#')
            .map(|(_, url)| url.trim())
            .unwrap_or(line);
        let Some(path) = extract_clean_alist_path(raw) else {
            return;
        };
        let Some((folder, file)) = path.rsplit_once('/') else {
            return;
        };
        let Some((_, extension)) = file.rsplit_once('.') else {
            return;
        };
        if matches!(
            extension.to_ascii_lowercase().as_str(),
            "srt" | "ass" | "ssa" | "vtt"
        ) {
            self.folders
                .entry(folder.to_string())
                .or_default()
                .push(path);
        }
    }

    pub fn attach(&mut self, records: &mut [ParsedStrmRecord]) {
        for paths in self.folders.values_mut() {
            paths.sort();
            paths.dedup();
        }
        for record in records {
            let Some((folder, file)) = record.alist_path.rsplit_once('/') else {
                continue;
            };
            let stem = file.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(file);
            record.subtitles = self
                .folders
                .get(folder)
                .into_iter()
                .flatten()
                .filter(|path| {
                    let name = path.rsplit('/').next().unwrap_or_default();
                    let sub_stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
                    sub_stem == stem
                        || sub_stem
                            .strip_prefix(stem)
                            .is_some_and(|suffix| suffix.starts_with('.'))
                })
                .cloned()
                .collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::parser::parse_strm_line;

    #[test]
    fn external_subtitles_match_folder_and_stem_in_any_order() {
        let lines = [
            "/电影/片名/Film.zh.srt#https://alist/d/media/Film.zh.srt?sign=abc",
            "/电影/片名/Film.mkv#https://alist/d/media/Film.mkv",
            "/media/Film.ass",
            "/media/Film.ssa",
            "/media/Film.vtt",
            "/other/Film.srt",
            "/media/Film2.srt",
            "/media/Film.txt",
        ];
        let mut index = SubtitleIndex::default();
        let mut records = Vec::new();
        for line in lines {
            index.add_line(line);
            if let Some(record) = parse_strm_line(line) {
                records.push(record);
            }
        }
        index.attach(&mut records);
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].subtitles,
            vec![
                "/media/Film.ass",
                "/media/Film.ssa",
                "/media/Film.vtt",
                "/media/Film.zh.srt"
            ]
        );
    }
}
