use std::{
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use vibeterm::watch::ConfigWatcher;
#[test]
fn creation_invalid_edit_and_atomic_replacement_are_observed() {
    let root = std::env::temp_dir().join(format!(
        "vibeterm-watch-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("new-directory/config.toml");
    let (tx, rx) = mpsc::channel();
    let watcher = ConfigWatcher::start(path.clone(), move |result| {
        tx.send(result).unwrap();
    })
    .unwrap();
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "font_size = 18").unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap()
            .font_size,
        18.0
    );
    std::fs::write(&path, "font_size = 0").unwrap();
    assert!(rx.recv_timeout(Duration::from_secs(3)).unwrap().is_err());
    let replacement = path.with_extension("tmp");
    std::fs::write(&replacement, "font_size = 22").unwrap();
    std::fs::rename(replacement, &path).unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap()
            .font_size,
        22.0
    );
    assert!(
        rx.recv_timeout(Duration::from_millis(250)).is_err(),
        "Unchanged file should not trigger periodic reloads"
    );
    // Editors and configuration managers can replace the entire parent directory.
    std::fs::rename(path.parent().unwrap(), root.join("old-directory")).unwrap();
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "font_size = 24").unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap()
            .font_size,
        24.0
    );
    std::fs::write(&path, "font_size = 25").unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap()
            .font_size,
        25.0
    );
    drop(watcher);
    std::fs::remove_dir_all(root).unwrap();
}
