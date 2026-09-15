use super::*;

use crate::auth::session::random_token;

const DEV_SHA256: &str = "ef260e9aa3c673af240d17a2660480361a8e081d1ffeca2a5ed0e3219fc18567";

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sccr-store-{}", random_token()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn sample() -> Store {
    let mut bindings = BTreeMap::new();
    bindings.insert(
        "C0AB12CD3:1789560000.000100".to_string(),
        Binding {
            session: "macbook:proj".into(),
            user: "U0123ABCD".into(),
            created: 1789560000,
        },
    );
    Store {
        users: vec!["U0123ABCD".into()],
        tokens: vec![TokenRecord {
            sha256: DEV_SHA256.into(),
            label: "macbook".into(),
            created: "2026-09-16T00:00:00Z".into(),
        }],
        bindings,
    }
}

#[test]
fn missing_file_is_default() {
    let path = temp_dir().join("absent.json");
    let store = Store::load(&path).unwrap();
    assert!(store.users.is_empty());
    assert!(store.tokens.is_empty());
    assert!(store.bindings.is_empty());
}

#[test]
fn roundtrip_file() {
    let path = temp_dir().join("state.json");
    let store = sample();
    store.save(&path).unwrap();
    let loaded = Store::load(&path).unwrap();
    assert_eq!(loaded, store);
}

#[test]
fn readme_example_without_created_loads() {
    let path = temp_dir().join("readme.json");
    fs::write(
        &path,
        r#"{"users":["U0123"],"tokens":[{"sha256":"abc","label":"macbook","created":"2026-09-16T00:00:00Z"}],"bindings":{"C0AB:1700000000.000100":{"session":"macbook:proj","user":"U0123"}},"extra":1}"#,
    )
    .unwrap();
    let loaded = Store::load(&path).unwrap();
    assert_eq!(loaded.bindings["C0AB:1700000000.000100"].created, 0);
}

#[test]
fn invalid_json_is_error() {
    let path = temp_dir().join("bad.json");
    fs::write(&path, "{not json").unwrap();
    assert!(Store::load(&path).is_err());
}

#[test]
fn save_leaves_no_tmp() {
    let dir = temp_dir();
    let path = dir.join("state.json");
    sample().save(&path).unwrap();
    Store::default().save(&path).unwrap();
    let names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["state.json"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn token_ok_matches_sha256_only() {
    let store = sample();
    assert!(store.token_ok("dev"));
    assert!(!store.token_ok(DEV_SHA256));
    assert!(!store.token_ok(""));
    assert!(!store.token_ok("dev "));
    assert!(!Store::default().token_ok("dev"));
}
