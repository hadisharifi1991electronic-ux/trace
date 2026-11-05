//! Electrical constraints in the style of Allegro's Constraint Manager:
//! length limits, matched-length groups, pair skew and via counts, set on
//! groups of nets and checked against the board.

use crate::{Board, Length};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// Which nets a group covers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Members {
    /// Every net.
    AllNets,
    /// Every differential pair.
    AllPairs,
    /// A net class from the board.
    NetClass(String),
    /// A differential pair class from the board.
    PairClass(String),
    /// Nets whose names match a pattern (`*` any run, `?` one character).
    Pattern(String),
    /// Pairs whose names match a pattern.
    PairPattern(String),
    /// Named nets.
    Nets(Vec<String>),
}

impl Members {
    pub fn describe(&self) -> String {
        match self {
            Members::AllNets => "all nets".into(),
            Members::AllPairs => "all differential pairs".into(),
            Members::NetClass(c) => format!("net class {c}"),
            Members::PairClass(c) => format!("pair class {c}"),
            Members::Pattern(p) => format!("nets {p}"),
            Members::PairPattern(p) => format!("pairs {p}"),
            Members::Nets(n) if n.len() <= 3 => n.join(", "),
            Members::Nets(n) => format!("{} nets", n.len()),
        }
    }

    /// Whether the members are pairs (skew applies inside each one).
    pub fn is_pairs(&self) -> bool {
        matches!(
            self,
            Members::AllPairs | Members::PairClass(_) | Members::PairPattern(_)
        )
    }
}

/// Limits for one group. Unset limits are not checked.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    pub min_length: Option<Length>,
    pub max_length: Option<Length>,
    /// All members within this much of the longest one.
    pub match_tolerance: Option<Length>,
    /// The two nets of each pair within this much of each other.
    pub max_skew: Option<Length>,
    pub max_vias: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub name: String,
    pub members: Members,
    pub limits: Limits,
    pub enabled: bool,
    /// Where the group came from, such as the Altium rule it was read from.
    #[serde(default)]
    pub source: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConstraintSet {
    pub groups: Vec<Group>,
}

/// One net's measured values and how it fares against its group.
#[derive(Clone, Debug)]
pub struct NetResult {
    pub net: u32,
    pub name: String,
    /// The pair this net belongs to, when the group is of pairs.
    pub pair: Option<String>,
    pub length: Length,
    pub vias: usize,
    /// Shortfall against the longest net of a matched group (always >= 0).
    pub match_delta: Option<Length>,
    /// Length difference to its pair partner.
    pub skew: Option<Length>,
    pub problems: Vec<Problem>,
    /// Not fully routed: lengths are not final.
    pub open: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Problem {
    TooShort,
    TooLong,
    Unmatched,
    Skew,
    TooManyVias,
}

impl Problem {
    pub fn label(self) -> &'static str {
        match self {
            Problem::TooShort => "too short",
            Problem::TooLong => "too long",
            Problem::Unmatched => "outside match",
            Problem::Skew => "pair skew",
            Problem::TooManyVias => "too many vias",
        }
    }
}

#[derive(Clone, Debug)]
pub struct GroupResult {
    pub nets: Vec<NetResult>,
    /// Longest member, the target of a matched group.
    pub target: Option<Length>,
    pub failing: usize,
}

fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_ascii_uppercase().chars().collect();
    let t: Vec<char> = text.to_ascii_uppercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

impl Board {
    /// Signal length of a net: tracks, arcs and via barrels.
    pub fn net_length(&self, net: u32) -> Length {
        self.nets
            .get(net as usize)
            .map_or(Length(0), |n| Length(n.routed_length.0 + n.via_length.0))
    }

    /// The nets a group covers, with the pair each belongs to.
    pub fn members(&self, m: &Members) -> Vec<(u32, Option<String>)> {
        let by_name: HashMap<&str, u32> = self
            .nets
            .iter()
            .enumerate()
            .map(|(i, n)| (n.name.as_str(), i as u32))
            .collect();
        let pairs = |pick: &dyn Fn(&crate::DiffPair) -> bool| {
            let mut out = Vec::new();
            for p in self.diff_pairs.iter().filter(|p| pick(p)) {
                for n in [&p.positive, &p.negative] {
                    if let Some(&i) = by_name.get(n.as_str()) {
                        out.push((i, Some(p.name.clone())));
                    }
                }
            }
            out
        };
        let nets = |pick: &dyn Fn(&str) -> bool| -> Vec<(u32, Option<String>)> {
            self.nets
                .iter()
                .enumerate()
                .filter(|(_, n)| !n.name.is_empty() && pick(&n.name))
                .map(|(i, _)| (i as u32, None))
                .collect()
        };
        let mut out = match m {
            Members::AllNets => nets(&|_| true),
            Members::AllPairs => pairs(&|_| true),
            Members::NetClass(c) => {
                let Some(class) = self.net_classes.iter().find(|k| &k.name == c) else {
                    return Vec::new();
                };
                if class.members.is_empty() && class.name.eq_ignore_ascii_case("All Nets") {
                    return nets(&|_| true);
                }
                nets(&|n| class.members.iter().any(|m| m == n))
            }
            Members::PairClass(c) => {
                let Some(class) = self.diff_pair_classes.iter().find(|k| &k.name == c) else {
                    return Vec::new();
                };
                if class.members.is_empty() {
                    return pairs(&|_| true);
                }
                pairs(&|p| class.members.iter().any(|m| m == &p.name))
            }
            Members::Pattern(p) => nets(&|n| glob(p, n)),
            Members::PairPattern(p) => pairs(&|d| glob(p, &d.name)),
            Members::Nets(list) => list
                .iter()
                .filter_map(|n| by_name.get(n.as_str()).map(|&i| (i, None)))
                .collect(),
        };
        // Nets whose pair partner is in the group too are checked as a pair.
        let inside: std::collections::HashSet<u32> = out.iter().map(|(n, _)| *n).collect();
        for p in &self.diff_pairs {
            let (Some(&a), Some(&b)) = (
                by_name.get(p.positive.as_str()),
                by_name.get(p.negative.as_str()),
            ) else {
                continue;
            };
            if inside.contains(&a) && inside.contains(&b) {
                for (n, pair) in out.iter_mut() {
                    if (*n == a || *n == b) && pair.is_none() {
                        *pair = Some(p.name.clone());
                    }
                }
            }
        }
        out
    }

    /// Measures a group's nets and checks them against its limits.
    pub fn check(&self, g: &Group) -> GroupResult {
        let members = self.members(&g.members);
        let lim = &g.limits;
        let len_of: HashMap<u32, Length> = members
            .iter()
            .map(|(n, _)| (*n, self.net_length(*n)))
            .collect();
        let target = members.iter().map(|(n, _)| len_of[n]).max();
        // Partner lengths for skew.
        let mut by_pair: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
        for (n, p) in &members {
            if let Some(p) = p {
                by_pair.entry(p.as_str()).or_default().push(*n);
            }
        }
        let mut nets = Vec::with_capacity(members.len());
        for (n, pair) in &members {
            let net = &self.nets[*n as usize];
            let length = len_of[n];
            let skew = pair.as_deref().and_then(|p| {
                let other = by_pair.get(p)?.iter().find(|o| *o != n)?;
                Some(Length((length.0 - len_of[other].0).abs()))
            });
            let match_delta = lim
                .match_tolerance
                .and(target)
                .map(|t| Length(t.0 - length.0));
            let mut problems = Vec::new();
            if g.enabled {
                if lim.min_length.is_some_and(|m| length < m) {
                    problems.push(Problem::TooShort);
                }
                if lim.max_length.is_some_and(|m| length > m) {
                    problems.push(Problem::TooLong);
                }
                if let (Some(tol), Some(d)) = (lim.match_tolerance, match_delta) {
                    if d > tol {
                        problems.push(Problem::Unmatched);
                    }
                }
                if let (Some(max), Some(s)) = (lim.max_skew, skew) {
                    if s > max {
                        problems.push(Problem::Skew);
                    }
                }
                if lim.max_vias.is_some_and(|m| net.vias > m) {
                    problems.push(Problem::TooManyVias);
                }
            }
            nets.push(NetResult {
                net: *n,
                name: net.name.clone(),
                pair: pair.clone(),
                length,
                vias: net.vias,
                match_delta,
                skew,
                open: net.pieces > 1 && !net.has_pour,
                problems,
            });
        }
        let failing = nets.iter().filter(|r| !r.problems.is_empty()).count();
        GroupResult {
            nets,
            target,
            failing,
        }
    }

    /// Starting constraints from the board's own design rules (Altium
    /// Length and MatchedLengths rules). Rules whose scope is not
    /// understood are left out.
    pub fn imported_constraints(&self) -> ConstraintSet {
        let mut set = ConstraintSet::default();
        for r in &self.rules {
            let len = |k: &str| r.params.get(k).and_then(|v| Length::parse(v));
            let Some(members) = r.scope.first().and_then(|s| parse_scope(s)) else {
                continue;
            };
            let mut limits = Limits::default();
            match r.kind.as_str() {
                "Length" => {
                    limits.min_length = len("MINLIMIT").filter(|l| l.0 > 0);
                    limits.max_length = len("MAXLIMIT");
                }
                "MatchedLengths" => {
                    let tol = len("TOLERANCE");
                    let flag = |k: &str| {
                        r.params
                            .get(k)
                            .is_some_and(|v| v.eq_ignore_ascii_case("TRUE"))
                    };
                    if flag("CHECKNETSINDIFFPAIR") {
                        limits.max_skew = tol;
                    }
                    if flag("CHECKOTHERS") || flag("CHECKDIFFPAIRVSDIFFPAIR") {
                        limits.match_tolerance = tol;
                    }
                }
                _ => continue,
            }
            if limits == Limits::default() {
                continue;
            }
            set.groups.push(Group {
                name: r.name.clone(),
                members,
                limits,
                enabled: r.enabled,
                source: format!("Altium {} rule", r.kind),
            });
        }
        set
    }
}

/// Reads simple Altium rule scopes: `All`, `InNetClass('x')`,
/// `InDifferentialPairClass('x')`, `InNet('x')`, `InDifferentialPair('x')`,
/// `InxSignalClass('x')`.
fn parse_scope(s: &str) -> Option<Members> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("All") {
        return Some(Members::AllNets);
    }
    let open = s.find('(')?;
    let func = s[..open].trim().to_ascii_lowercase();
    let arg = s[open + 1..]
        .strip_suffix(')')?
        .trim()
        .trim_matches('\'')
        .to_string();
    if arg.contains('\'') {
        return None; // more than one argument or a compound expression
    }
    Some(match func.as_str() {
        "innetclass" | "inxsignalclass" if arg.eq_ignore_ascii_case("All Nets") => Members::AllNets,
        "innetclass" | "inxsignalclass" => Members::NetClass(arg),
        "indifferentialpairclass" if arg.eq_ignore_ascii_case("All Differential Pairs") => {
            Members::AllPairs
        }
        "indifferentialpairclass" => Members::PairClass(arg),
        "innet" => Members::Nets(vec![arg]),
        "indifferentialpair" => Members::PairPattern(arg),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob("RX*_P", "rx1_p"));
        assert!(glob("TX?", "TX3"));
        assert!(!glob("TX?", "TX10"));
        assert!(glob("*", ""));
    }

    #[test]
    fn scopes() {
        assert_eq!(
            parse_scope("InNetClass('QSPI')"),
            Some(Members::NetClass("QSPI".into()))
        );
        assert_eq!(
            parse_scope("InDifferentialPairClass('All Differential Pairs')"),
            Some(Members::AllPairs)
        );
        assert_eq!(parse_scope("InNet('A') or InNet('B')"), None);
    }
}
