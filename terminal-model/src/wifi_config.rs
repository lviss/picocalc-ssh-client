//! Storage encoding and migration logic for saved WiFi networks.
//!
//! The firmware's flash config store (`src/config.rs`) is a flat string
//! key/value map with 32-character keys and 128-character values. Earlier
//! versions of this firmware stored exactly one network under two keys,
//! `wifi_ssid` and `wifi_pw`, read once at boot (`src/net.rs::setup_wifi`).
//! This module adds support for several saved networks instead of one:
//!
//! * each saved network is one `wifi_net_<name>` key, holding both its SSID
//!   and password joined by a unit-separator byte (`\x1f`) that can't appear
//!   in console input, since `src/process.rs` splits commands on literal
//!   ASCII space characters and the console never produces control
//!   characters;
//! * `wifi_active` holds the name of the network to join at boot, and the
//!   one most recently selected with `wifi use <name>`.
//!
//! This module only holds the pure, host-testable parts - key naming, the
//! value codec, and the one-time migration decision away from the old
//! `wifi_ssid`/`wifi_pw` keys - per AGENTS.md's "host-testable logic lives
//! in terminal-model" guidance. `src/net.rs` is the flash/console adapter
//! that drives it and performs the actual radio join.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Config key holding the name of the network to use.
pub const ACTIVE_KEY: &str = "wifi_active";
/// Prefix for a saved network's config key, e.g. `wifi_net_home`.
pub const NETWORK_KEY_PREFIX: &str = "wifi_net_";
/// Legacy single-network config keys this module migrates away from.
pub const LEGACY_SSID_KEY: &str = "wifi_ssid";
pub const LEGACY_PASSWORD_KEY: &str = "wifi_pw";
/// Name a migrated legacy `wifi_ssid`/`wifi_pw` pair is saved under.
pub const MIGRATED_NAME: &str = "default";

/// Config keys are 32-character fixed strings (see `src/config.rs::StrKey`),
/// so a saved network's name must leave room for [`NETWORK_KEY_PREFIX`].
pub const MAX_NAME_LEN: usize = 32 - NETWORK_KEY_PREFIX.len();

/// Separates the SSID and password within one stored value. Not typeable at
/// the console (`src/process.rs` splits commands on literal space
/// characters), so it can never collide with real SSID/password input.
const FIELD_SEP: char = '\u{1f}';

/// Builds the config key a network named `name` is stored under, or `None`
/// if `name` is empty or won't fit a 32-character config key alongside
/// [`NETWORK_KEY_PREFIX`].
pub fn network_key(name: &str) -> Option<String> {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return None;
    }
    Some(format!("{NETWORK_KEY_PREFIX}{name}"))
}

/// Recovers a saved network's name from its config key, e.g.
/// `network_name("wifi_net_home") == Some("home")`. Returns `None` for any
/// other key, so callers can filter the full config key set (as returned by
/// `config list`) down to just the saved networks.
pub fn network_name(key: &str) -> Option<&str> {
    key.strip_prefix(NETWORK_KEY_PREFIX)
}

/// Encodes an SSID and password into the single string a network's config
/// value holds. An empty `password` represents an open (unsecured) network.
pub fn encode_network(ssid: &str, password: &str) -> String {
    format!("{ssid}{FIELD_SEP}{password}")
}

/// Decodes a stored network value back into its `(ssid, password)` parts.
/// Returns `None` for a value that doesn't contain the separator, which
/// cannot happen for a value this module wrote.
pub fn decode_network(value: &str) -> Option<(&str, &str)> {
    value.split_once(FIELD_SEP)
}

/// Splits a `wifi` console command line into tokens. `src/process.rs`
/// tokenizes every other command with a plain `str::split(' ')`, which has
/// no way to put a space inside a single argument - fatal for `wifi save
/// <name> <ssid> [password]`, since real-world SSIDs and passwords commonly
/// contain spaces. This tokenizer keeps the same unquoted behavior (runs of
/// spaces separate tokens) but additionally treats a `"..."` run as a single
/// token, spaces and all, with the quotes stripped - e.g. `save home "Pizza
/// Palace Guest" "my pass"` yields `["save", "home", "Pizza Palace Guest",
/// "my pass"]`. An unterminated trailing quote takes the rest of the line
/// as that token's content rather than erroring. Scoped to the `wifi`
/// command only; every other command keeps using the shared unquoted
/// tokenizer.
pub fn tokenize_command(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = line.chars().peekable();
    while chars.peek().is_some() {
        while chars.peek() == Some(&' ') {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut token = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                token.push(c);
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ' ' {
                    break;
                }
                token.push(c);
                chars.next();
            }
        }
        tokens.push(token);
    }
    tokens
}

/// What to do, if anything, with a device's config store the first time it
/// boots a firmware version that understands a network list.
#[derive(Debug, PartialEq, Eq)]
pub enum Migration {
    /// Nothing to do: either a network list already exists (this device has
    /// already migrated, or was never on the old single-network scheme), or
    /// there's no legacy credential to migrate.
    NotNeeded,
    /// Store `ssid`/`password` as the [`MIGRATED_NAME`] network, make it
    /// active, and remove the legacy keys.
    Migrate { ssid: String, password: String },
}

/// Decides whether a legacy `wifi_ssid`/`wifi_pw` pair should be migrated
/// into the new network-list scheme. Takes plain booleans/options rather
/// than touching the config store itself, so the decision is host-testable;
/// `src/net.rs` does the actual flash reads/writes this describes.
pub fn plan_migration(
    has_active: bool,
    has_any_saved_network: bool,
    legacy_ssid: Option<&str>,
    legacy_password: Option<&str>,
) -> Migration {
    // A device that already has a network list (from this version, or from
    // `wifi save`/`wifi use` run since) must never have it silently reset
    // from leftover legacy keys.
    if has_active || has_any_saved_network {
        return Migration::NotNeeded;
    }
    match (legacy_ssid, legacy_password) {
        (Some(ssid), Some(password)) if !ssid.is_empty() => Migration::Migrate {
            ssid: String::from(ssid),
            password: String::from(password),
        },
        _ => Migration::NotNeeded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_command_splits_plain_unquoted_words() {
        assert_eq!(
            tokenize_command("wifi save home myssid mypassword"),
            ["wifi", "save", "home", "myssid", "mypassword"]
        );
    }

    #[test]
    fn tokenize_command_keeps_a_quoted_ssid_with_spaces_as_one_token() {
        assert_eq!(
            tokenize_command(r#"wifi save cafe "Pizza Palace Guest" mypassword"#),
            ["wifi", "save", "cafe", "Pizza Palace Guest", "mypassword"]
        );
    }

    #[test]
    fn tokenize_command_keeps_a_quoted_password_with_spaces_as_one_token() {
        assert_eq!(
            tokenize_command(r#"wifi save cafe "Pizza Palace Guest" "my pass""#),
            ["wifi", "save", "cafe", "Pizza Palace Guest", "my pass"]
        );
    }

    #[test]
    fn tokenize_command_treats_empty_quotes_as_an_empty_token() {
        assert_eq!(
            tokenize_command(r#"wifi save cafe "Pizza Palace Guest" """#),
            ["wifi", "save", "cafe", "Pizza Palace Guest", ""]
        );
    }

    #[test]
    fn tokenize_command_takes_rest_of_line_for_an_unterminated_quote() {
        assert_eq!(
            tokenize_command(r#"wifi save cafe "Pizza Palace Guest"#),
            ["wifi", "save", "cafe", "Pizza Palace Guest"]
        );
    }

    #[test]
    fn tokenize_command_collapses_runs_of_spaces() {
        assert_eq!(
            tokenize_command("wifi  save   home"),
            ["wifi", "save", "home"]
        );
    }

    #[test]
    fn network_key_round_trips_the_name() {
        let key = network_key("home").unwrap();
        assert_eq!(key, "wifi_net_home");
        assert_eq!(network_name(&key), Some("home"));
    }

    #[test]
    fn network_key_rejects_empty_and_overlong_names() {
        assert_eq!(network_key(""), None);
        let max = "a".repeat(MAX_NAME_LEN);
        assert!(network_key(&max).is_some());
        let too_long = "a".repeat(MAX_NAME_LEN + 1);
        assert_eq!(network_key(&too_long), None);
    }

    #[test]
    fn network_name_ignores_unrelated_keys() {
        assert_eq!(network_name("wifi_active"), None);
        assert_eq!(network_name("ssh_user"), None);
        assert_eq!(network_name("host_home"), None);
    }

    #[test]
    fn encode_decode_round_trips_ssid_and_password() {
        let encoded = encode_network("MyNetwork", "hunter2");
        assert_eq!(decode_network(&encoded), Some(("MyNetwork", "hunter2")));
    }

    #[test]
    fn encode_decode_round_trips_an_open_network() {
        let encoded = encode_network("OpenNet", "");
        assert_eq!(decode_network(&encoded), Some(("OpenNet", "")));
    }

    #[test]
    fn decode_rejects_a_value_with_no_separator() {
        assert_eq!(decode_network("not-a-valid-value"), None);
    }

    #[test]
    fn migration_is_not_needed_once_a_network_list_exists() {
        assert_eq!(
            plan_migration(true, false, Some("Net"), Some("pw")),
            Migration::NotNeeded
        );
        assert_eq!(
            plan_migration(false, true, Some("Net"), Some("pw")),
            Migration::NotNeeded
        );
    }

    #[test]
    fn migration_is_not_needed_without_legacy_credentials() {
        assert_eq!(
            plan_migration(false, false, None, None),
            Migration::NotNeeded
        );
        assert_eq!(
            plan_migration(false, false, Some("Net"), None),
            Migration::NotNeeded
        );
        assert_eq!(
            plan_migration(false, false, Some(""), Some("pw")),
            Migration::NotNeeded
        );
    }

    #[test]
    fn migration_moves_a_fresh_legacy_pair_into_the_list() {
        assert_eq!(
            plan_migration(false, false, Some("Net"), Some("pw")),
            Migration::Migrate {
                ssid: String::from("Net"),
                password: String::from("pw"),
            }
        );
    }

    #[test]
    fn migration_accepts_a_legacy_network_with_no_password() {
        assert_eq!(
            plan_migration(false, false, Some("Net"), Some("")),
            Migration::Migrate {
                ssid: String::from("Net"),
                password: String::from(""),
            }
        );
    }
}
