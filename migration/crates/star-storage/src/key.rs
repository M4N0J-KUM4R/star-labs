//! Canvas-compatible keys for attachment bytes.
//!
//! Canvas does not store a "storage key" column: it derives the key of an
//! attachment's bytes from the attachment id (and the file name) in the
//! backend that holds them.
//!
//! * file system backend — `gems/attachment_fu/lib/attachment_fu/backends/file_system_backend.rb`
//!   * `full_filename` (line 38) = `Rails.root.join(path_prefix, *partitioned_path(name))`
//!   * `partitioned_path` (line 57) = `("%08d" % attachment_path_id).scan(/..../) + args`
//!   * `attachment_path_id` — `app/models/attachment.rb:1287` — the root
//!     attachment's id.
//!   * `path_prefix` comes from `config/file_store.yml` (`tmp/files`).
//!   * so attachment 569 named `report.pdf` lives at
//!     `tmp/files/0000/0569/report.pdf`.
//!
//! * S3 backend — `gems/attachment_fu/lib/attachment_fu/backends/s3_backend.rb`
//!   * `full_filename` (line 223) = `File.join(base_path, basename)`
//!   * `base_path` (line 217) = `File.join(namespaced_path, attachment_path_id)`
//!   * `namespaced_path` (line 206) = `File.join(namespace, path_prefix)`; the
//!     S3 `path_prefix` is the table name (`attachments`, see
//!     `gems/attachment_fu/lib/attachment_fu.rb:123`).
//!   * so attachment 569 named `report.pdf` lives at
//!     `<namespace>/attachments/569/report.pdf`.
//!
//! The port stores the key it used in `files.storage_key`, so a read never has
//! to guess where the bytes are; the key itself is still a pure function of
//! the attachment id and the file name, exactly as in Canvas.

/// Canvas `Attachment.truncate_filename` (`app/models/attachment.rb:1167`).
///
/// Keeps the extension and truncates the base name so the whole name fits
/// into `max_len` characters.
pub fn truncate_filename(filename: &str, max_len: usize) -> String {
    let ext_index = filename.rfind('.');
    match ext_index {
        Some(idx) => {
            let ext = &filename[idx..];
            let ext = truncate_component(ext, max_len / 2 + 1);
            let base = truncate_component(&filename[..idx], max_len.saturating_sub(ext.len()));
            format!("{base}{ext}")
        }
        None => truncate_component(filename, max_len),
    }
}

/// Canvas' component truncation: `str[0...len]`.
fn truncate_component(component: &str, len: usize) -> String {
    component.chars().take(len).collect()
}

/// Canvas' file-system `sanitize_filename`
/// (`gems/attachment_fu/lib/attachment_fu/backends/file_system_backend.rb:88`):
/// truncate to 255 characters, then replace `/` and spaces with `_`.
pub fn sanitize_filename(filename: &str) -> String {
    truncate_filename(filename, 255).replace(['/', ' '], "_")
}

/// The partitioned path Canvas puts in front of the file name:
/// `("%08d" % attachment_path_id).scan(/..../)`.
///
/// `569` becomes `0000/0569`, `12345678` becomes `1234/5678`.
pub fn partitioned_path(attachment_id: i64) -> String {
    let id = format!("{:08}", attachment_id.max(0));
    id.as_bytes()
        .chunks(4)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or("0000"))
        .collect::<Vec<_>>()
        .join("/")
}

/// The key Canvas' file-system backend uses, relative to its `path_prefix`.
///
/// `canvas_filesystem_key(569, "report.pdf")` -> `0000/0569/report.pdf`,
/// which Canvas keeps at `tmp/files/0000/0569/report.pdf`.
pub fn canvas_filesystem_key(attachment_id: i64, filename: &str) -> String {
    format!(
        "{}/{}",
        partitioned_path(attachment_id),
        sanitize_filename(filename)
    )
}

/// The key Canvas' S3 backend uses, relative to the bucket root.
///
/// `canvas_s3_key(569, "report.pdf", Some("account_1"))` ->
/// `account_1/attachments/569/report.pdf`; without a namespace it is
/// `attachments/569/report.pdf`.
pub fn canvas_s3_key(attachment_id: i64, filename: &str, namespace: Option<&str>) -> String {
    let mut key = String::new();
    if let Some(namespace) = namespace.filter(|ns| !ns.is_empty()) {
        key.push_str(namespace);
        key.push('/');
    }
    key.push_str("attachments");
    key.push('/');
    key.push_str(&attachment_id.to_string());
    key.push('/');
    key.push_str(&sanitize_filename(filename));
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_key_matches_canvas_layout() {
        // Canvas: tmp/files/0000/0569/report.pdf
        assert_eq!(
            canvas_filesystem_key(569, "report.pdf"),
            "0000/0569/report.pdf"
        );
        assert_eq!(
            canvas_filesystem_key(12_345_678, "a.txt"),
            "1234/5678/a.txt"
        );
        assert_eq!(canvas_filesystem_key(1, "a.txt"), "0000/0001/a.txt");
    }

    #[test]
    fn s3_key_matches_canvas_layout() {
        assert_eq!(
            canvas_s3_key(569, "report.pdf", Some("account_1")),
            "account_1/attachments/569/report.pdf"
        );
        assert_eq!(
            canvas_s3_key(569, "report.pdf", None),
            "attachments/569/report.pdf"
        );
    }

    #[test]
    fn filenames_are_sanitized_like_canvas() {
        assert_eq!(sanitize_filename("my report.pdf"), "my_report.pdf");
        assert_eq!(sanitize_filename("dir/report.pdf"), "dir_report.pdf");
        assert_eq!(truncate_filename("averylongname.txt", 10), "averyl.txt");
    }
}
