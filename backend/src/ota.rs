use crate::models::{OtaMeta, OtaStatusResponse, OtaUploadResponse, OtaValidation};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path};
use std::process::Command;
use std::sync::Mutex;

const OTA_STAGING_DIR: &str = "/tmp/ota_staging";
const OTA_BINARY_PATH: &str = "/home/root/udx710";
const OTA_WWW_PATH: &str = "/home/root/www";
const OTA_BACKUP_DIR: &str = "/home/root/.ota_backup";
const OTA_BACKUP_NEW_DIR: &str = "/home/root/.ota_backup.new";
const OTA_BACKUP_OLD_DIR: &str = "/home/root/.ota_backup.old";
const OTA_OLD_BINARY_PATH: &str = "/home/root/.udx710.ota-old";
const OTA_OLD_WWW_PATH: &str = "/home/root/.www.ota-old";
const OTA_NEW_BINARY_PATH: &str = "/home/root/.udx710.ota-new";
const OTA_NEW_WWW_PATH: &str = "/home/root/.www.ota-new";
const MAX_ARCHIVE_FILES: usize = 4096;
const MAX_EXTRACTED_BYTES: u64 = 200 * 1024 * 1024;
const MAX_PATH_DEPTH: usize = 16;
const TARGET_ARCH: &str = "aarch64-unknown-linux-musl";

static OTA_LOCK: Mutex<()> = Mutex::new(());

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn get_current_commit() -> String {
    option_env!("GIT_COMMIT").unwrap_or("unknown").to_string()
}

pub fn get_ota_status() -> OtaStatusResponse {
    let pending_meta = read_meta_from(Path::new(OTA_STAGING_DIR));
    let pending_validation = pending_meta
        .as_ref()
        .and_then(|meta| validate_ota_package(meta).ok());
    let rollback_meta = read_valid_rollback_meta();

    OtaStatusResponse {
        current_version: CURRENT_VERSION.to_string(),
        current_commit: get_current_commit(),
        pending_update: pending_meta.is_some(),
        pending_meta,
        pending_validation,
        rollback_available: rollback_meta.is_some(),
        rollback_meta,
    }
}

fn read_meta_from(root: &Path) -> Option<OtaMeta> {
    fs::read_to_string(root.join("meta.json"))
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
}

fn read_valid_rollback_meta() -> Option<OtaMeta> {
    let root = Path::new(OTA_BACKUP_DIR);
    let meta = read_meta_from(root)?;
    validate_payload(root, &meta, false)
        .ok()
        .filter(|validation| validation.valid)
        .map(|_| meta)
}

pub fn handle_ota_upload(data: &[u8]) -> Result<OtaUploadResponse, String> {
    let _guard = OTA_LOCK.lock().map_err(|_| "OTA lock is poisoned".to_string())?;

    if data.is_empty() {
        return Err("OTA package is empty".to_string());
    }
    if data.len() > 50 * 1024 * 1024 {
        return Err("OTA package exceeds the 50 MiB limit".to_string());
    }

    let _ = fs::remove_dir_all(OTA_STAGING_DIR);
    fs::create_dir_all(OTA_STAGING_DIR)
        .map_err(|e| format!("Failed to create staging dir: {}", e))?;

    if detect_zip_format(data) {
        let _ = fs::remove_dir_all(OTA_STAGING_DIR);
        return Err("Only .tar.gz OTA packages are accepted. Build and upload the GitHub release artifact.".to_string());
    }

    let archive_path = Path::new(OTA_STAGING_DIR).join("update.tar.gz");
    let mut file = fs::File::create(&archive_path)
        .map_err(|e| format!("Failed to create archive file: {}", e))?;
    file.write_all(data)
        .map_err(|e| format!("Failed to write archive file: {}", e))?;
    drop(file);

    let result = (|| {
        validate_archive_entries(&archive_path)?;
        extract_archive(&archive_path)?;
        let _ = fs::remove_file(&archive_path);
        validate_extracted_tree(Path::new(OTA_STAGING_DIR))?;
        fix_file_permissions(Path::new(OTA_STAGING_DIR))?;

        let meta = read_meta_from(Path::new(OTA_STAGING_DIR))
            .ok_or_else(|| "meta.json not found or invalid in OTA package".to_string())?;
        let validation = validate_ota_package(&meta)?;
        if !validation.valid {
            return Err(validation.error.unwrap_or_else(|| "OTA package validation failed".to_string()));
        }

        Ok(OtaUploadResponse { meta, validation })
    })();

    if result.is_err() {
        let _ = fs::remove_dir_all(OTA_STAGING_DIR);
    }
    result
}

fn validate_archive_entries(archive_path: &Path) -> Result<(), String> {
    let output = Command::new("tar")
        .args(["-tvzf", archive_path.to_str().unwrap_or_default()])
        .output()
        .map_err(|e| format!("Failed to inspect tar archive: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Failed to inspect archive: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let mut entries = HashSet::new();
    for raw_line in String::from_utf8_lossy(&output.stdout).lines() {
        let entry_type = raw_line.as_bytes().first().copied().unwrap_or_default();
        if entry_type != b'-' && entry_type != b'd' {
            return Err(format!("OTA archive contains forbidden entry type: {}", raw_line));
        }
        let raw_name = raw_line
            .split_whitespace()
            .last()
            .ok_or_else(|| format!("Failed to parse archive entry: {}", raw_line))?;
        let name = normalize_archive_name(raw_name)?;
        if name.is_empty() {
            continue;
        }
        if entries.len() >= MAX_ARCHIVE_FILES {
            return Err(format!("OTA archive contains more than {} entries", MAX_ARCHIVE_FILES));
        }
        if !is_allowed_archive_path(&name) {
            return Err(format!("OTA archive contains forbidden path: {}", name));
        }
        entries.insert(name);
    }

    if !entries.contains("meta.json") || !entries.contains("udx710") {
        return Err("OTA archive must contain meta.json and udx710".to_string());
    }
    if !entries.iter().any(|entry| entry == "www" || entry.starts_with("www/")) {
        return Err("OTA archive must contain a www directory".to_string());
    }
    Ok(())
}

fn normalize_archive_name(raw_name: &str) -> Result<String, String> {
    let trimmed = raw_name.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.contains('\\') {
        return Err(format!("OTA archive contains invalid path: {}", trimmed));
    }

    let mut normalized = trimmed;
    while let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped;
    }
    let path = Path::new(normalized);
    if path.is_absolute() || path.components().any(|component| matches!(component, Component::ParentDir)) {
        return Err(format!("OTA archive contains unsafe path: {}", trimmed));
    }
    if path.components().count() > MAX_PATH_DEPTH {
        return Err(format!("OTA archive path is too deep: {}", trimmed));
    }
    Ok(normalized.to_string())
}

fn is_allowed_archive_path(path: &str) -> bool {
    path == "meta.json" || path == "udx710" || path == "www" || path.starts_with("www/")
}

fn extract_archive(archive_path: &Path) -> Result<(), String> {
    let output = Command::new("tar")
        .args(["-xzf", archive_path.to_str().unwrap_or_default(), "-C", OTA_STAGING_DIR])
        .output()
        .map_err(|e| format!("Failed to extract tar archive: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Failed to extract archive: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn validate_extracted_tree(root: &Path) -> Result<(), String> {
    let mut file_count = 0usize;
    let mut total_bytes = 0u64;
    validate_tree_node(root, root, 0, &mut file_count, &mut total_bytes)?;

    for entry in fs::read_dir(root).map_err(|e| format!("Failed to read staging dir: {}", e))? {
        let path = entry.map_err(|e| format!("Failed to read staging entry: {}", e))?.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|e| format!("Failed to inspect staging path: {}", e))?;
        let name = relative.to_string_lossy();
        if name != "meta.json" && name != "udx710" && name != "www" {
            return Err(format!("OTA contains unexpected top-level entry: {}", name));
        }
    }
    Ok(())
}

fn validate_tree_node(
    root: &Path,
    path: &Path,
    depth: usize,
    file_count: &mut usize,
    total_bytes: &mut u64,
) -> Result<(), String> {
    if depth > MAX_PATH_DEPTH {
        return Err(format!("Extracted OTA directory is too deep: {}", path.display()));
    }

    let metadata = fs::symlink_metadata(path)
        .map_err(|e| format!("Failed to inspect extracted path {}: {}", path.display(), e))?;
    if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
        return Err(format!("OTA contains unsupported file type: {}", path.display()));
    }

    if metadata.is_file() {
        *file_count = file_count.saturating_add(1);
        *total_bytes = total_bytes.saturating_add(metadata.len());
        if *file_count > MAX_ARCHIVE_FILES || *total_bytes > MAX_EXTRACTED_BYTES {
            return Err("Extracted OTA exceeds resource limits".to_string());
        }
        return Ok(());
    }

    for entry in fs::read_dir(path).map_err(|e| format!("Failed to read {}: {}", path.display(), e))? {
        let child = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?.path();
        if child.strip_prefix(root).is_err() {
            return Err(format!("Extracted path escaped staging dir: {}", child.display()));
        }
        validate_tree_node(root, &child, depth + 1, file_count, total_bytes)?;
    }
    Ok(())
}

fn validate_ota_package(meta: &OtaMeta) -> Result<OtaValidation, String> {
    validate_payload(Path::new(OTA_STAGING_DIR), meta, true)
}

fn validate_payload(root: &Path, meta: &OtaMeta, check_min_version: bool) -> Result<OtaValidation, String> {
    let binary_path = root.join("udx710");
    let www_path = root.join("www");

    if !binary_path.is_file() {
        return Ok(invalid_validation("Binary file not found in package"));
    }
    if !www_path.is_dir() {
        return Ok(invalid_validation("Frontend directory not found in package"));
    }

    let binary_md5 = calculate_file_md5(&binary_path)?;
    let binary_md5_match = binary_md5.eq_ignore_ascii_case(&meta.binary_md5);
    let frontend_md5 = calculate_directory_md5(&www_path)?;
    let frontend_md5_match = frontend_md5.eq_ignore_ascii_case(&meta.frontend_md5);
    let arch_match = meta.arch == TARGET_ARCH && is_aarch64_elf(&binary_path)?;
    let version_relation = version_relation(&meta.version, CURRENT_VERSION)?;
    let is_newer = version_relation == "upgrade";
    let min_version_match = if check_min_version {
        meta.min_version
            .as_deref()
            .map(|minimum| version_is_at_least(CURRENT_VERSION, minimum))
            .transpose()?
            .unwrap_or(true)
    } else {
        true
    };

    let mut errors = Vec::new();
    if !binary_md5_match {
        errors.push(format!("Binary MD5 mismatch: expected={}, actual={}", meta.binary_md5, binary_md5));
    }
    if !frontend_md5_match {
        errors.push(format!("Frontend MD5 mismatch: expected={}, actual={}", meta.frontend_md5, frontend_md5));
    }
    if !arch_match {
        errors.push(format!("Architecture mismatch: expected={}, binary is not aarch64 ELF", TARGET_ARCH));
    }
    if !min_version_match {
        errors.push(format!(
            "Installed version {} is below package minimum {}",
            CURRENT_VERSION,
            meta.min_version.as_deref().unwrap_or("unknown")
        ));
    }

    Ok(OtaValidation {
        valid: errors.is_empty(),
        is_newer,
        binary_md5_match,
        frontend_md5_match,
        arch_match,
        version_relation,
        min_version_match,
        error: (!errors.is_empty()).then(|| errors.join("; ")),
    })
}

fn invalid_validation(error: &str) -> OtaValidation {
    OtaValidation {
        valid: false,
        is_newer: false,
        binary_md5_match: false,
        frontend_md5_match: false,
        arch_match: false,
        version_relation: "unknown".to_string(),
        min_version_match: false,
        error: Some(error.to_string()),
    }
}

fn is_aarch64_elf(path: &Path) -> Result<bool, String> {
    let mut file = fs::File::open(path)
        .map_err(|e| format!("Failed to open binary {}: {}", path.display(), e))?;
    let mut header = [0u8; 20];
    file.read_exact(&mut header)
        .map_err(|e| format!("Failed to read ELF header: {}", e))?;
    if &header[0..4] != b"\x7fELF" || header[4] != 2 {
        return Ok(false);
    }
    let machine = match header[5] {
        1 => u16::from_le_bytes([header[18], header[19]]),
        2 => u16::from_be_bytes([header[18], header[19]]),
        _ => return Ok(false),
    };
    Ok(machine == 183)
}

fn calculate_file_md5(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path)
        .map_err(|e| format!("Failed to open file {}: {}", path.display(), e))?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)
        .map_err(|e| format!("Failed to read file {}: {}", path.display(), e))?;
    Ok(format!("{:x}", md5::compute(&contents)))
}

fn collect_directory_hashes(path: &Path, hashes: &mut Vec<String>) -> Result<(), String> {
    let entries = fs::read_dir(path)
        .map_err(|e| format!("Failed to read directory {}: {}", path.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to read directory entry: {}", e))?;
        let entry_path = entry.path();
        let metadata = fs::symlink_metadata(&entry_path)
            .map_err(|e| format!("Failed to inspect {}: {}", entry_path.display(), e))?;
        if metadata.is_dir() {
            collect_directory_hashes(&entry_path, hashes)?;
        } else if metadata.is_file() {
            hashes.push(calculate_file_md5(&entry_path)?);
        } else {
            return Err(format!("Unsupported frontend file type: {}", entry_path.display()));
        }
    }
    Ok(())
}

fn calculate_directory_md5(path: &Path) -> Result<String, String> {
    let mut hashes = Vec::new();
    collect_directory_hashes(path, &mut hashes)?;
    hashes.sort();
    let mut payload = hashes.join("\n");
    if !payload.is_empty() {
        payload.push('\n');
    }
    Ok(format!("{:x}", md5::compute(payload.as_bytes())))
}

fn parse_version(version: &str) -> Result<Vec<u32>, String> {
    let trimmed = version.trim();
    if trimmed.is_empty() {
        return Err("Version must not be empty".to_string());
    }
    let parts: Result<Vec<u32>, _> = trimmed.split('.').map(|part| part.parse::<u32>()).collect();
    let parts = parts.map_err(|_| format!("Invalid semantic version: {}", version))?;
    if parts.len() < 2 || parts.len() > 4 {
        return Err(format!("Invalid semantic version: {}", version));
    }
    Ok(parts)
}

fn version_relation(candidate: &str, installed: &str) -> Result<String, String> {
    let candidate_parts = parse_version(candidate)?;
    let installed_parts = parse_version(installed)?;
    for index in 0..std::cmp::max(candidate_parts.len(), installed_parts.len()) {
        let candidate_part = candidate_parts.get(index).copied().unwrap_or(0);
        let installed_part = installed_parts.get(index).copied().unwrap_or(0);
        if candidate_part > installed_part {
            return Ok("upgrade".to_string());
        }
        if candidate_part < installed_part {
            return Ok("downgrade".to_string());
        }
    }
    Ok("same".to_string())
}

fn version_is_at_least(candidate: &str, installed: &str) -> Result<bool, String> {
    Ok(version_relation(candidate, installed)? != "downgrade")
}

pub fn apply_ota_update(restart_now: bool, allow_downgrade: bool) -> Result<String, String> {
    let _guard = OTA_LOCK.lock().map_err(|_| "OTA lock is poisoned".to_string())?;

    let meta = read_meta_from(Path::new(OTA_STAGING_DIR)).ok_or_else(|| "No pending update".to_string())?;
    let validation = validate_ota_package(&meta)?;
    if !validation.valid {
        return Err(validation.error.unwrap_or_else(|| "OTA package validation failed".to_string()));
    }
    if validation.version_relation != "upgrade" && !allow_downgrade {
        let operation = if validation.version_relation == "same" {
            "same-version reinstall"
        } else {
            "downgrade"
        };
        return Err(format!(
            "Package version {} requires explicit confirmation for {} from installed version {}. Set allow_downgrade to apply it.",
            meta.version, operation, CURRENT_VERSION
        ));
    }

    let staging_binary = Path::new(OTA_STAGING_DIR).join("udx710");
    let staging_www = Path::new(OTA_STAGING_DIR).join("www");
    install_update(&staging_binary, &staging_www)?;
    let _ = fs::remove_dir_all(OTA_STAGING_DIR);

    if restart_now {
        schedule_reboot();
    }

    Ok(format!("Update to version {} applied successfully", meta.version))
}

pub fn rollback_ota_update(restart_now: bool) -> Result<String, String> {
    let _guard = OTA_LOCK.lock().map_err(|_| "OTA lock is poisoned".to_string())?;
    let backup_root = Path::new(OTA_BACKUP_DIR);
    let meta = read_valid_rollback_meta().ok_or_else(|| "No valid rollback snapshot is available".to_string())?;
    install_update(&backup_root.join("udx710"), &backup_root.join("www"))?;

    if restart_now {
        schedule_reboot();
    }
    Ok(format!("Rollback to version {} applied successfully", meta.version))
}

fn schedule_reboot() {
    let _ = crate::restart::schedule_reboot("ota", 1);
}

fn install_update(staging_binary: &Path, staging_www: &Path) -> Result<(), String> {
    clean_install_temporary_paths();
    prepare_backup_snapshot()?;

    fs::copy(staging_binary, OTA_NEW_BINARY_PATH)
        .map_err(|e| format!("Failed to stage binary: {}", e))?;
    copy_dir_recursive(staging_www, Path::new(OTA_NEW_WWW_PATH))?;
    set_file_mode(Path::new(OTA_NEW_BINARY_PATH), 0o755)?;
    fix_file_permissions(Path::new(OTA_NEW_WWW_PATH))?;

    let had_binary = Path::new(OTA_BINARY_PATH).exists();
    let had_www = Path::new(OTA_WWW_PATH).exists();
    let install_result = (|| {
        if had_binary {
            fs::rename(OTA_BINARY_PATH, OTA_OLD_BINARY_PATH)
                .map_err(|e| format!("Failed to move current binary: {}", e))?;
        }
        if had_www {
            fs::rename(OTA_WWW_PATH, OTA_OLD_WWW_PATH)
                .map_err(|e| format!("Failed to move current frontend: {}", e))?;
        }
        fs::rename(OTA_NEW_BINARY_PATH, OTA_BINARY_PATH)
            .map_err(|e| format!("Failed to install new binary: {}", e))?;
        fs::rename(OTA_NEW_WWW_PATH, OTA_WWW_PATH)
            .map_err(|e| format!("Failed to install new frontend: {}", e))?;
        crate::config::ensure_loader_hooks_init()?;
        publish_backup_snapshot()?;
        Ok::<(), String>(())
    })();

    if let Err(error) = install_result {
        rollback_live_install(had_binary, had_www);
        let _ = fs::remove_dir_all(OTA_BACKUP_NEW_DIR);
        return Err(error);
    }

    let _ = fs::remove_file(OTA_OLD_BINARY_PATH);
    let _ = fs::remove_dir_all(OTA_OLD_WWW_PATH);
    Ok(())
}

fn clean_install_temporary_paths() {
    let _ = fs::remove_file(OTA_NEW_BINARY_PATH);
    let _ = fs::remove_dir_all(OTA_NEW_WWW_PATH);
    let _ = fs::remove_file(OTA_OLD_BINARY_PATH);
    let _ = fs::remove_dir_all(OTA_OLD_WWW_PATH);
    let _ = fs::remove_dir_all(OTA_BACKUP_NEW_DIR);
    let _ = fs::remove_dir_all(OTA_BACKUP_OLD_DIR);
}

fn prepare_backup_snapshot() -> Result<(), String> {
    let live_binary = Path::new(OTA_BINARY_PATH);
    let live_www = Path::new(OTA_WWW_PATH);
    if !live_binary.is_file() || !live_www.is_dir() {
        return Err("Cannot create rollback snapshot because the current installation is incomplete".to_string());
    }

    fs::create_dir_all(OTA_BACKUP_NEW_DIR)
        .map_err(|e| format!("Failed to create rollback snapshot directory: {}", e))?;
    fs::copy(live_binary, Path::new(OTA_BACKUP_NEW_DIR).join("udx710"))
        .map_err(|e| format!("Failed to snapshot current binary: {}", e))?;
    copy_dir_recursive(live_www, &Path::new(OTA_BACKUP_NEW_DIR).join("www"))?;
    set_file_mode(&Path::new(OTA_BACKUP_NEW_DIR).join("udx710"), 0o755)?;
    fix_file_permissions(&Path::new(OTA_BACKUP_NEW_DIR).join("www"))?;

    let meta = current_install_meta()?;
    write_meta_atomically(Path::new(OTA_BACKUP_NEW_DIR), &meta)?;
    Ok(())
}

fn current_install_meta() -> Result<OtaMeta, String> {
    Ok(OtaMeta {
        version: CURRENT_VERSION.to_string(),
        commit: get_current_commit(),
        build_time: "rollback-snapshot".to_string(),
        binary_md5: calculate_file_md5(Path::new(OTA_BINARY_PATH))?,
        frontend_md5: calculate_directory_md5(Path::new(OTA_WWW_PATH))?,
        arch: TARGET_ARCH.to_string(),
        min_version: None,
    })
}

fn write_meta_atomically(root: &Path, meta: &OtaMeta) -> Result<(), String> {
    let temporary = root.join("meta.json.new");
    let final_path = root.join("meta.json");
    let content = serde_json::to_vec_pretty(meta)
        .map_err(|e| format!("Failed to serialize rollback metadata: {}", e))?;
    fs::write(&temporary, content)
        .map_err(|e| format!("Failed to write rollback metadata: {}", e))?;
    fs::rename(&temporary, &final_path)
        .map_err(|e| format!("Failed to publish rollback metadata: {}", e))?;
    set_file_mode(&final_path, 0o644)
}

fn publish_backup_snapshot() -> Result<(), String> {
    let backup = Path::new(OTA_BACKUP_DIR);
    let backup_new = Path::new(OTA_BACKUP_NEW_DIR);
    let backup_old = Path::new(OTA_BACKUP_OLD_DIR);
    if backup.exists() {
        fs::rename(backup, backup_old)
            .map_err(|e| format!("Failed to preserve existing rollback snapshot: {}", e))?;
    }
    if let Err(error) = fs::rename(backup_new, backup) {
        if backup_old.exists() {
            let _ = fs::rename(backup_old, backup);
        }
        return Err(format!("Failed to publish rollback snapshot: {}", error));
    }
    let _ = fs::remove_dir_all(backup_old);
    Ok(())
}

fn rollback_live_install(had_binary: bool, had_www: bool) {
    let _ = fs::remove_file(OTA_BINARY_PATH);
    let _ = fs::remove_dir_all(OTA_WWW_PATH);
    let _ = fs::remove_file(OTA_NEW_BINARY_PATH);
    let _ = fs::remove_dir_all(OTA_NEW_WWW_PATH);
    if had_binary {
        let _ = fs::rename(OTA_OLD_BINARY_PATH, OTA_BINARY_PATH);
    }
    if had_www {
        let _ = fs::rename(OTA_OLD_WWW_PATH, OTA_WWW_PATH);
    }
    let _ = crate::config::ensure_loader_hooks_init();
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst)
        .map_err(|e| format!("Failed to create dir {}: {}", dst.display(), e))?;
    for entry in fs::read_dir(src).map_err(|e| format!("Failed to read src dir {}: {}", src.display(), e))? {
        let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
        let src_path = entry.path();
        let metadata = fs::symlink_metadata(&src_path)
            .map_err(|e| format!("Failed to inspect {}: {}", src_path.display(), e))?;
        let dst_path = dst.join(entry.file_name());
        if metadata.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if metadata.is_file() {
            fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("Failed to copy file {}: {}", src_path.display(), e))?;
        } else {
            return Err(format!("Unsupported file type in OTA tree: {}", src_path.display()));
        }
    }
    Ok(())
}

pub fn cancel_pending_update() -> Result<(), String> {
    let _guard = OTA_LOCK.lock().map_err(|_| "OTA lock is poisoned".to_string())?;
    if Path::new(OTA_STAGING_DIR).exists() {
        fs::remove_dir_all(OTA_STAGING_DIR)
            .map_err(|e| format!("Failed to remove staging dir: {}", e))?;
    }
    Ok(())
}

fn detect_zip_format(data: &[u8]) -> bool {
    data.len() >= 4 && data[0..4] == [0x50, 0x4B, 0x03, 0x04]
}

fn set_file_mode(path: &Path, mode: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .map_err(|e| format!("Failed to read metadata for {}: {}", path.display(), e))?
            .permissions();
        permissions.set_mode(mode);
        fs::set_permissions(path, permissions)
            .map_err(|e| format!("Failed to set permissions for {}: {}", path.display(), e))?;
    }
    Ok(())
}

fn fix_file_permissions(root: &Path) -> Result<(), String> {
    if !root.exists() {
        return Err(format!("OTA path does not exist: {}", root.display()));
    }
    for entry in fs::read_dir(root).map_err(|e| format!("Failed to read {}: {}", root.display(), e))? {
        let path = entry.map_err(|e| format!("Failed to read permission entry: {}", e))?.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|e| format!("Failed to inspect {}: {}", path.display(), e))?;
        if metadata.is_dir() {
            fix_file_permissions(&path)?;
            set_file_mode(&path, 0o755)?;
        } else if metadata.is_file() {
            set_file_mode(&path, 0o644)?;
        } else {
            return Err(format!("Unsupported OTA file type: {}", path.display()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        is_allowed_archive_path, normalize_archive_name, version_is_at_least, version_relation,
    };

    #[test]
    fn versions_classify_numerically() {
        assert_eq!(version_relation("3.10.0", "3.9.0").unwrap(), "upgrade");
        assert_eq!(version_relation("3.4.0", "3.4.0").unwrap(), "same");
        assert_eq!(version_relation("3.3.9", "3.4.0").unwrap(), "downgrade");
        assert!(version_is_at_least("3.4", "3.4.0").unwrap());
    }

    #[test]
    fn archive_paths_are_restricted() {
        assert_eq!(normalize_archive_name("./www/index.html").unwrap(), "www/index.html");
        assert!(normalize_archive_name("../etc/passwd").is_err());
        assert!(normalize_archive_name("/etc/passwd").is_err());
        assert!(is_allowed_archive_path("www/assets/app.js"));
        assert!(!is_allowed_archive_path("etc/passwd"));
    }
}
