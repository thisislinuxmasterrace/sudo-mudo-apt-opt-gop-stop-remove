use super::*;

#[test]
fn file_and_directory_are_weak_and_report_unknowns() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file");
    std::fs::write(&file, b"data").unwrap();
    let file_assessment = Assessment::for_path(&file, &std::fs::symlink_metadata(&file).unwrap());
    assert!(file_assessment.line().starts_with("weak:"));
    let mut fields = String::new();
    file_assessment.write_fields(&mut fields);
    // the value depends on the filesystem under the temporary directory.
    assert!(fields.contains("\"compression\":\""));
    assert!(fields.contains("\"snapshots\":\"unknown\""));
    assert!(fields.contains("\"other_copies\":\"none_found\",\"copies\":[]"));
    assert!(file_assessment.line().contains("others may exist"));
    let directory =
        Assessment::for_path(dir.path(), &std::fs::symlink_metadata(dir.path()).unwrap());
    assert!(directory.line().contains("metadata only"));
    assert!(directory.line().contains("coverage may be incomplete"));
}

#[cfg(unix)]
#[test]
fn dangling_link_does_not_inspect_target() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink("missing", &link).unwrap();
    let assessment = Assessment::for_path(&link, &std::fs::symlink_metadata(&link).unwrap());
    assert!(assessment.line().starts_with("cosmetic:"));
    assert_eq!(assessment.filesystem, None);
    assert_eq!(assessment.medium, "unknown");
}

#[cfg(target_os = "linux")]
#[test]
fn replaced_path_does_not_supply_facts_for_selected_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("file");
    std::fs::write(&file, b"data").unwrap();
    let meta = std::fs::symlink_metadata(&file).unwrap();
    std::fs::rename(&file, dir.path().join("old")).unwrap();
    std::os::unix::fs::symlink("/", &file).unwrap();
    let assessment = Assessment::for_path(&file, &meta);
    assert_eq!(assessment.filesystem, None);
    assert_eq!(assessment.medium, "unknown");
    assert_eq!(assessment.verdict, "weak");
}

#[test]
fn unreadable_object_is_unknown() {
    let assessment = Assessment::unknown();
    assert!(assessment.line().contains("coverage is incomplete"));
    assert_eq!(assessment.medium, "unknown");
}

#[cfg(unix)]
#[test]
fn special_objects_do_not_supply_storage_facts() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("fifo");
    nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRUSR).unwrap();
    let assessment = Assessment::for_path(&fifo, &std::fs::symlink_metadata(&fifo).unwrap());
    assert_eq!(assessment.filesystem, None);
    assert_eq!(assessment.medium, "unknown");
    assert!(
        assessment
            .line()
            .contains("object contents are not assessed")
    );
}

#[test]
fn found_copies_are_listed_and_never_raise_the_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, b"data").unwrap();
    std::fs::write(dir.path().join("notes.txt~"), b"old").unwrap();
    let assessment = Assessment::for_path(&file, &std::fs::symlink_metadata(&file).unwrap());
    assert_eq!(assessment.verdict, "weak");
    assert!(assessment.line().contains("editor backup"));
    let mut fields = String::new();
    assessment.write_fields(&mut fields);
    assert!(fields.contains("\"other_copies\":\"found\""), "{fields}");
    assert!(
        fields.contains("{\"kind\":\"editor_backup\",\"path\":"),
        "{fields}"
    );
    assert!(fields.contains("notes.txt~"), "{fields}");
}

#[test]
fn uncertain_facts_never_produce_effective() {
    for filesystem in [
        None,
        Some("ext4"),
        Some("btrfs"),
        Some("nfs"),
        Some("other"),
    ] {
        for medium in ["unknown", "flash", "rotational"] {
            for encryption in [
                Encryption::NotChecked,
                Encryption::NoneDetected,
                Encryption::DmCrypt,
                Encryption::Ecryptfs,
            ] {
                let assessment = Assessment::from_facts(
                    filesystem.map(str::to_owned),
                    medium,
                    Compression::Unknown,
                    encryption,
                    false,
                    true,
                    None,
                );
                assert_eq!(assessment.verdict, "weak");
                assert!(
                    assessment
                        .line()
                        .contains("compression and snapshots are unknown")
                );
            }
        }
    }
}

#[test]
fn encryption_is_a_fact_and_a_reason_and_never_a_verdict() {
    let assessment = Assessment::from_facts(
        Some("ext4".to_owned()),
        "flash",
        Compression::Unknown,
        Encryption::DmCrypt,
        false,
        true,
        None,
    );
    let mut fields = String::new();
    assessment.write_fields(&mut fields);
    assert!(fields.contains("\"encryption\":\"dm_crypt\""), "{fields}");
    assert!(
        assessment
            .line()
            .contains("destroying every copy of the key")
    );
    assert_eq!(assessment.verdict, "weak");
    let plain = Assessment::from_facts(
        None,
        "unknown",
        Compression::Unknown,
        Encryption::NotChecked,
        false,
        true,
        None,
    );
    let mut fields = String::new();
    plain.write_fields(&mut fields);
    assert!(
        fields.contains("\"encryption\":\"not_checked\""),
        "{fields}"
    );
}

#[test]
fn compression_is_a_fact_and_a_reason_and_never_a_verdict() {
    for (compression, text, reason) in [
        (Compression::Enabled, "enabled", "smaller extents"),
        (
            Compression::NotSupported,
            "not_supported",
            "no transparent compression",
        ),
        (
            Compression::Unknown,
            "unknown",
            "compression and snapshots are unknown",
        ),
    ] {
        let assessment = Assessment::from_facts(
            Some("btrfs".to_owned()),
            "flash",
            compression,
            Encryption::NotChecked,
            false,
            true,
            None,
        );
        let mut fields = String::new();
        assessment.write_fields(&mut fields);
        assert!(
            fields.contains(&format!("\"compression\":\"{text}\"")),
            "{fields}"
        );
        assert!(assessment.line().contains(reason), "{}", assessment.line());
        assert_eq!(assessment.verdict, "weak");
        assert!(assessment.line().contains("snapshots are unknown"));
    }
}
