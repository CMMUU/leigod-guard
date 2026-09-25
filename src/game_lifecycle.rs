//! Pure game observations: no account, HTTP, UI, or platform side effects.
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct ProcessId {
    pub pid: u32,
    pub created: Option<u64>,
    pub exe: String,
}

impl ProcessId {
    fn same_instance(&self, other: &Self) -> bool {
        self.pid == other.pid
            && self.exe.eq_ignore_ascii_case(&other.exe)
            && (self.created == other.created || self.created.is_none() || other.created.is_none())
    }
}

#[derive(Clone, Debug)]
pub struct Process {
    pub id: ProcessId,
    pub age: Option<Duration>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Unknown,
    Absent,
    Launching,
    Running,
}

#[derive(Clone, Debug)]
pub struct Observation {
    pub phase: Phase,
    pub at: Instant,
    pub generation: u64,
    pub activity_seen: bool,
    pub running: Vec<String>,
    pub launching: Vec<String>,
    pub launch_until: Option<Instant>,
    pub processes: Vec<String>,
    pub evidence: Vec<ProcessId>,
}

impl Observation {
    pub fn unknown(now: Instant) -> Self {
        Self {
            phase: Phase::Unknown,
            at: now,
            generation: 0,
            activity_seen: false,
            running: Vec::new(),
            launching: Vec::new(),
            launch_until: None,
            processes: Vec::new(),
            evidence: Vec::new(),
        }
    }

    pub fn remaining(&self, now: Instant) -> u64 {
        self.launch_until.map_or(0, |until| {
            let left = until.saturating_duration_since(now);
            left.as_secs() + u64::from(left.subsec_nanos() > 0)
        })
    }

    pub fn game_running(&self) -> Option<bool> {
        match self.phase {
            Phase::Running => Some(true),
            Phase::Absent => Some(false),
            Phase::Unknown | Phase::Launching => None,
        }
    }
}

#[derive(Default)]
struct Game {
    known: HashSet<ProcessId>,
    running: bool,
    activity_seen: bool,
    // Retain an exhausted episode while helpers remain. Child handoffs and
    // respawns in that episode cannot repeatedly buy another protection window.
    launch_until: Option<Instant>,
}

#[derive(Default)]
pub struct Tracker {
    games: HashMap<String, Game>,
    watch: Vec<(String, String)>,
    generation: u64,
}

/// These are preparation signals, NOT extra always-running game executables.
pub fn launchers(exe: &str) -> &'static [&'static str] {
    if exe.eq_ignore_ascii_case("TslGame.exe") {
        &["ExecPubg.exe", "TslGame_ZK.exe", "TslGame_BE.exe"]
    } else {
        &[]
    }
}

impl Tracker {
    pub fn observe(
        &mut self,
        now: Instant,
        processes: &[Process],
        watch: &[(String, String)],
        launch_secs: u64,
    ) -> Observation {
        if self.watch != watch {
            self.games.clear();
            self.watch = watch.to_vec();
            self.generation = self.generation.saturating_add(1);
        }
        let mut out = Observation::unknown(now);
        out.phase = Phase::Absent;
        out.processes = processes.iter().map(|p| p.id.exe.clone()).collect();
        // Bound deadlines even if a hand-edited configuration contains u64::MAX.
        let budget = Duration::from_secs(launch_secs.clamp(30, 3600));
        for (name, exe) in watch {
            let game = self.games.entry(exe.to_ascii_lowercase()).or_default();
            let related: Vec<_> = processes
                .iter()
                .filter(|p| {
                    p.id.exe.eq_ignore_ascii_case(exe)
                        || launchers(exe)
                            .iter()
                            .any(|n| p.id.exe.eq_ignore_ascii_case(n))
                })
                .collect();
            let running = related.iter().any(|p| p.id.exe.eq_ignore_ascii_case(exe));
            out.evidence.extend(related.iter().map(|p| p.id.clone()));
            if running {
                if !game.running {
                    self.generation = self.generation.saturating_add(1);
                }
                game.activity_seen = true;
                game.launch_until = None;
                out.running.push(name.clone());
            } else {
                let new_budget = related
                    .iter()
                    .filter(|p| !game.known.iter().any(|old| old.same_instance(&p.id)))
                    .map(|p| budget.saturating_sub(p.age.unwrap_or(Duration::ZERO)))
                    .max()
                    .unwrap_or(Duration::ZERO);
                if game.launch_until.is_none() && !new_budget.is_zero() {
                    game.launch_until = Some(now + new_budget);
                    game.activity_seen = true;
                    self.generation = self.generation.saturating_add(1);
                }
                if let Some(until) = game.launch_until {
                    if now < until {
                        out.launching.push(name.clone());
                        out.launch_until = Some(out.launch_until.map_or(until, |u| u.max(until)));
                    } else if related.is_empty() {
                        // A fully absent, completed attempt may be followed by
                        // an intentional new attempt. Existing helpers are consumed.
                        game.launch_until = None;
                    }
                }
            }
            game.running = running;
            game.known = related.iter().map(|p| p.id.clone()).collect();
            out.activity_seen |= game.activity_seen;
        }
        out.generation = self.generation;
        out.phase = if !out.running.is_empty() {
            Phase::Running
        } else if !out.launching.is_empty() {
            Phase::Launching
        } else {
            Phase::Absent
        };
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, exe: &str) -> Process {
        Process {
            id: ProcessId {
                pid,
                created: Some(pid as u64),
                exe: exe.into(),
            },
            age: Some(Duration::ZERO),
        }
    }
    fn watch() -> Vec<(String, String)> {
        vec![("PUBG".into(), "TslGame.exe".into())]
    }

    #[test]
    fn september_25_launch_is_protected_before_main_process_appears() {
        let t = Instant::now();
        let mut state = Tracker::default();
        assert_eq!(state.observe(t, &[], &watch(), 600).phase, Phase::Absent);
        let root = p(1, "ExecPubg.exe");
        assert_eq!(
            state
                .observe(t + Duration::from_secs(172), &[root.clone()], &watch(), 600)
                .phase,
            Phase::Launching
        );
        assert_eq!(
            state
                .observe(t + Duration::from_secs(181), &[root.clone()], &watch(), 600)
                .phase,
            Phase::Launching
        );
        assert_eq!(
            state
                .observe(
                    t + Duration::from_secs(211),
                    &[root, p(2, "TslGame.exe")],
                    &watch(),
                    600
                )
                .phase,
            Phase::Running
        );
    }

    #[test]
    fn child_handoff_and_respawns_do_not_renew_a_failed_attempt() {
        let t = Instant::now();
        let mut state = Tracker::default();
        let first = state.observe(t, &[p(1, "ExecPubg.exe")], &watch(), 600);
        let second = state.observe(
            t + Duration::from_secs(590),
            &[p(2, "TslGame_ZK.exe")],
            &watch(),
            600,
        );
        assert_eq!(first.launch_until, second.launch_until);
        assert_eq!(first.generation, second.generation);
        assert_eq!(
            state
                .observe(
                    t + Duration::from_secs(601),
                    &[p(3, "TslGame_BE.exe")],
                    &watch(),
                    600
                )
                .phase,
            Phase::Absent
        );
    }

    #[test]
    fn leftover_helpers_after_a_game_cannot_rearm_launch_protection() {
        let t = Instant::now();
        let mut state = Tracker::default();
        let root = p(1, "ExecPubg.exe");
        state.observe(t, &[root.clone(), p(2, "TslGame.exe")], &watch(), 600);
        assert_eq!(
            state
                .observe(t + Duration::from_secs(3), &[root.clone()], &watch(), 600)
                .phase,
            Phase::Absent
        );
        assert_eq!(
            state
                .observe(t + Duration::from_secs(100), &[root], &watch(), 600)
                .phase,
            Phase::Absent
        );
        assert_eq!(
            state
                .observe(
                    t + Duration::from_secs(101),
                    &[p(4, "ExecPubg.exe")],
                    &watch(),
                    600
                )
                .phase,
            Phase::Launching
        );
    }

    #[test]
    fn late_attachment_does_not_refresh_an_old_launcher() {
        let t = Instant::now();
        let mut old = p(1, "ExecPubg.exe");
        old.age = Some(Duration::from_secs(1000));
        assert_eq!(
            Tracker::default().observe(t, &[old], &watch(), 600).phase,
            Phase::Absent
        );
        let mut recent = p(2, "ExecPubg.exe");
        recent.age = Some(Duration::from_secs(580));
        assert_eq!(
            Tracker::default()
                .observe(t, &[recent], &watch(), 600)
                .remaining(t),
            20
        );
    }

    #[test]
    fn main_name_is_sufficient_even_if_metadata_is_denied() {
        let t = Instant::now();
        let mut main = p(1, "TSLGAME.EXE");
        main.id.created = None;
        main.age = None;
        assert_eq!(
            Tracker::default().observe(t, &[main], &watch(), 600).phase,
            Phase::Running
        );
        assert_eq!(
            Tracker::default()
                .observe(t, &[p(2, "steam.exe")], &watch(), 600)
                .phase,
            Phase::Absent
        );
    }

    #[test]
    fn metadata_denial_does_not_turn_leftovers_into_a_new_launch() {
        let t = Instant::now();
        let mut state = Tracker::default();
        let mut root = p(1, "ExecPubg.exe");
        state.observe(t, &[root.clone(), p(2, "TslGame.exe")], &watch(), 600);
        root.id.created = None;
        root.age = None;
        assert_eq!(
            state.observe(t, &[root], &watch(), 600).phase,
            Phase::Absent
        );
    }

    #[test]
    fn pid_reuse_with_a_new_creation_time_is_a_new_instance() {
        let t = Instant::now();
        let mut state = Tracker::default();
        let mut root = p(1, "ExecPubg.exe");
        state.observe(t, &[root.clone(), p(2, "TslGame.exe")], &watch(), 600);
        state.observe(t, &[root.clone()], &watch(), 600);
        root.id.created = Some(1000);
        assert_eq!(
            state.observe(t, &[root], &watch(), 600).phase,
            Phase::Launching
        );
    }

    #[test]
    fn another_running_game_wins_over_launch_and_unknown_is_not_false() {
        let t = Instant::now();
        let mut games = watch();
        games.push(("Other".into(), "other.exe".into()));
        let out =
            Tracker::default().observe(t, &[p(1, "ExecPubg.exe"), p(2, "other.exe")], &games, 600);
        assert_eq!(out.game_running(), Some(true));
        assert_eq!(Observation::unknown(t).game_running(), None);
        assert_eq!(
            Tracker::default()
                .observe(t, &[p(1, "ExecPubg.exe")], &games, 600)
                .game_running(),
            None
        );
    }
}
