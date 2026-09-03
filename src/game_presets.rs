//! Small, editable defaults for real game processes, never launcher/anti-cheat
//! processes. References identify executable names, not compatibility guarantees.
//! Checked 2026-09-03; regions and future game versions may use different names.

#[derive(Clone, Copy, Debug)]
pub struct GamePreset {
    pub name: &'static str,
    pub exe: &'static str,
    pub source: &'static str,
}

pub const PRESETS: &[GamePreset] = &[
    GamePreset {
        name: "PUBG：绝地求生",
        exe: "TslGame.exe",
        source: "https://support.pubg.com/hc/en-us/articles/115004167294-How-to-resolve-TslGame-exe-error",
    },
    // First-hand Windows crash/launch logs in Valve's issue tracker; this is not
    // a Valve support/compatibility certification.
    GamePreset {
        name: "Counter-Strike 2（CS2）",
        exe: "cs2.exe",
        source: "https://github.com/ValveSoftware/csgo-osx-linux/issues/4216",
    },
    GamePreset {
        name: "Apex Legends",
        exe: "r5apex.exe",
        source: "https://help.ea.com/en/articles/apex-legends/error-codes/",
    },
    // This is the match process, not LeagueClient.exe/RiotClientServices.exe.
    GamePreset {
        name: "英雄联盟（对局）",
        exe: "League of Legends.exe",
        source: "https://support-leagueoflegends.riotgames.com/hc/en-us/articles/4407290569747-Advanced-Connections-Troubleshooting-Guide",
    },
    // Riot's firewall article lists the Shipping executable stem:
    // https://support-valorant.riotgames.com/hc/en-us/articles/360048522893-Your-Firewall-VS-VALORANT
    // NVIDIA's first-party FrameView capture example confirms its full .exe name.
    GamePreset {
        name: "无畏契约（国际服）",
        exe: "VALORANT-Win64-Shipping.exe",
        source: "https://images.nvidia.com/content/geforce/technologies/frameview/frameview-1-7-user-guide-web-version.pdf",
    },
    GamePreset {
        name: "堡垒之夜",
        exe: "FortniteClient-Win64-Shipping.exe",
        source: "https://www.epicgames.com/help/c-202300000001639/c-202300000001736/a202300000016932?lang=en-US",
    },
    GamePreset {
        name: "火箭联盟",
        exe: "RocketLeague.exe",
        source: "https://www.epicgames.com/help/c-202300000001622/c-202300000001748/a202300000011223?lang=en-US",
    },
    // First-hand Overwatch 2 crash logs on the publisher's support forum name
    // _retail_/Overwatch.exe; this is not a publisher compatibility guarantee.
    GamePreset {
        name: "守望先锋",
        exe: "Overwatch.exe",
        source: "https://eu.forums.blizzard.com/en/overwatch/t/black-screen-crash-on-launch/29114",
    },
];

/// Match the monitor's lowercase comparison, ignoring accidental input padding.
/// Display names do not participate: one process should only be watched once.
pub fn contains_executable<'a>(existing: impl IntoIterator<Item = &'a str>, exe: &str) -> bool {
    let candidate = exe.trim().to_lowercase();
    existing
        .into_iter()
        .any(|value| value.trim().to_lowercase() == candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_contains_unique_valid_single_executables() {
        let mut seen = Vec::new();
        for preset in PRESETS {
            assert!(
                crate::config::valid_game_executable(preset.exe),
                "{}",
                preset.exe
            );
            assert!(
                !contains_executable(seen.iter().copied(), preset.exe),
                "duplicate {}",
                preset.exe
            );
            assert!(!preset.name.trim().is_empty());
            assert!(preset.source.starts_with("https://"));
            seen.push(preset.exe);
        }
    }

    #[test]
    fn duplicates_ignore_case_and_padding_but_not_whole_names() {
        let existing = [" TslGame.exe ", "League of Legends.exe", "自定义É.exe"];
        assert!(contains_executable(existing, "tslgame.EXE"));
        assert!(contains_executable(existing, " league OF legends.EXE "));
        assert!(contains_executable(existing, "自定义é.exe"));
        assert!(!contains_executable(existing, "TslGame_BE.exe"));
        assert!(!contains_executable(existing, "Other.exe"));
    }
}
