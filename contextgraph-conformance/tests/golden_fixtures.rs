use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use contextgraph_conformance::check_frames;
use contextgraph_types::{ContextFrame, ContextQuery, ContextQueryResult, PROTOCOL_VERSION};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

const PROFILE: &str = "contextgraph-1.0-draft";
const PROFILE_VERSION: &str = "1.0.0";
const GENERATION_COMMAND: &str = "cargo test -p contextgraph-conformance --test golden_fixtures";
const FIXTURE_FILES: [&str; 4] = [
    "context-frame.missing-citation.invalid.json",
    "context-frame.valid.json",
    "context-query.valid.json",
    "normalization-vectors.json",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    protocol_version: String,
    fixture_profile_version: String,
    generation_command: String,
    files: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameFixtures {
    fully_populated: Value,
    minimal: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryFixture {
    query: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvalidFrameFixture {
    frame: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NormalizationFixtures {
    vectors: Vec<NormalizationVector>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NormalizationVector {
    name: String,
    input_json: String,
    expected_normalized: Value,
    expected_jcs_utf8: String,
    sha256: String,
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(PROFILE)
}

fn read_fixture<T: for<'de> Deserialize<'de>>(name: &str) -> T {
    let path = fixture_dir().join(name);
    let bytes = fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn sha256_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

fn assert_lowercase_sha256(digest: &str) {
    let hex = digest
        .strip_prefix("sha256:")
        .unwrap_or_else(|| panic!("digest lacks sha256: prefix: {digest}"));
    assert_eq!(hex.len(), 64, "digest must contain 32 bytes: {digest}");
    assert!(
        hex.bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "digest must use lowercase hexadecimal: {digest}"
    );
}

fn fixture_file_names(directory: &Path) -> BTreeSet<String> {
    fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()))
        .map(|entry| entry.expect("fixture directory entry must be readable"))
        .filter(|entry| {
            entry
                .file_type()
                .expect("fixture file type must be readable")
                .is_file()
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn manifest_has_strict_coverage_and_correct_file_hashes() {
    let directory = fixture_dir();
    let manifest: Manifest = read_fixture("manifest.json");

    assert_eq!(manifest.protocol_version, PROTOCOL_VERSION);
    assert_eq!(manifest.fixture_profile_version, PROFILE_VERSION);
    assert_eq!(manifest.generation_command, GENERATION_COMMAND);

    let declared: BTreeSet<_> = manifest.files.keys().cloned().collect();
    let expected: BTreeSet<_> = FIXTURE_FILES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(
        declared, expected,
        "manifest must declare every profile fixture"
    );

    let mut on_disk = fixture_file_names(&directory);
    assert!(on_disk.remove("manifest.json"));
    assert_eq!(
        on_disk, declared,
        "manifest coverage must exactly match disk"
    );

    for (name, expected_digest) in manifest.files {
        assert_lowercase_sha256(&expected_digest);
        let bytes = fs::read(directory.join(&name))
            .unwrap_or_else(|error| panic!("failed to read {name}: {error}"));
        assert_eq!(sha256_digest(&bytes), expected_digest, "hash for {name}");
    }
}

#[test]
fn valid_frames_deserialize_and_preserve_full_and_minimal_wire_shapes() {
    let fixtures: FrameFixtures = read_fixture("context-frame.valid.json");
    let full: ContextFrame = serde_json::from_value(fixtures.fully_populated.clone())
        .expect("fully populated frame must deserialize");
    let minimal: ContextFrame =
        serde_json::from_value(fixtures.minimal.clone()).expect("minimal frame must deserialize");

    assert_eq!(
        serde_json::to_value(&full).unwrap(),
        fixtures.fully_populated
    );
    assert_eq!(serde_json::to_value(&minimal).unwrap(), fixtures.minimal);
    assert_eq!(full.provenance.len(), 1);
    assert_eq!(full.relations.len(), 1);
    assert!(full.embedding.is_some());
    assert!(minimal.provenance.is_empty());
    assert!(minimal.relations.is_empty());

    let result = ContextQueryResult {
        frames: vec![full, minimal],
        truncated: false,
        dropped_estimate: None,
    };
    let (passed, evidence) = check_frames(&result);
    assert!(passed, "valid golden frames were rejected: {evidence}");
}

#[test]
fn query_omitted_arrays_deserialize_to_defaults_and_stay_omitted() {
    let fixture: QueryFixture = read_fixture("context-query.valid.json");
    let query: ContextQuery =
        serde_json::from_value(fixture.query.clone()).expect("valid query must deserialize");

    assert!(query.kinds.is_empty());
    assert!(query.anchors.is_empty());
    assert_eq!(serde_json::to_value(&query).unwrap(), fixture.query);
}

#[test]
fn blank_citation_fixture_is_rejected_by_frame_conformance() {
    let fixture: InvalidFrameFixture = read_fixture("context-frame.missing-citation.invalid.json");
    let frame: ContextFrame =
        serde_json::from_value(fixture.frame).expect("invalid fixture must still match wire shape");
    let result = ContextQueryResult {
        frames: vec![frame],
        truncated: false,
        dropped_estimate: None,
    };

    let (passed, evidence) = check_frames(&result);
    assert!(!passed, "blank citation unexpectedly passed conformance");
    assert!(
        evidence.contains("citation_label"),
        "unexpected evidence: {evidence}"
    );
}

#[test]
fn normalization_vectors_match_rfc_8785_text_and_sha256() {
    let fixtures: NormalizationFixtures = read_fixture("normalization-vectors.json");
    let names: BTreeSet<_> = fixtures
        .vectors
        .iter()
        .map(|vector| vector.name.as_str())
        .collect();
    assert_eq!(names, BTreeSet::from(["escaping", "numbers", "unicode"]));

    for vector in fixtures.vectors {
        let input: Value = serde_json::from_str(&vector.input_json)
            .unwrap_or_else(|error| panic!("{} input is invalid JSON: {error}", vector.name));
        assert_eq!(
            input, vector.expected_normalized,
            "{} normalized JSON value",
            vector.name
        );

        // JCS is computed by an RFC 8785 implementation. Ordinary serde JSON
        // serialization is used above only for typed wire-shape assertions.
        let canonical = serde_json_canonicalizer::to_string(&input)
            .unwrap_or_else(|error| panic!("{} could not be canonicalized: {error}", vector.name));
        assert_eq!(
            canonical, vector.expected_jcs_utf8,
            "{} JCS text",
            vector.name
        );

        assert_lowercase_sha256(&vector.sha256);
        assert_eq!(
            sha256_digest(vector.expected_jcs_utf8.as_bytes()),
            vector.sha256,
            "{} JCS digest",
            vector.name
        );
    }
}
