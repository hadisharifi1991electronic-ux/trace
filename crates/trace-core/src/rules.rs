//! The design rules as an editor sees them, in the manner of Altium's PCB
//! Rules and Constraints Editor: every rule kind in its category with the
//! fields it takes, new rules with sensible defaults, priorities kept in
//! order within a kind, scopes checked as they are typed, and the rule
//! that wins for a net (width, clearance, via style, pair sizes) for the
//! interactive router.
//!
//! Rules stay as `Rule`s (kind, name, scopes, Altium parameters) so a
//! PcbDoc reads and writes back without loss. Priority 1 is the highest;
//! of the enabled rules of a kind whose scopes take an object, the one
//! with the smallest priority number applies.

use crate::drc::{Obj, Scopes, Thing};
use crate::{Board, Length, Rule};
use std::collections::BTreeMap;

/// The groups rule kinds are listed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Electrical,
    Routing,
    Smt,
    Mask,
    Plane,
    Testpoint,
    Manufacturing,
    HighSpeed,
    Placement,
    SignalIntegrity,
    Other,
}

impl Category {
    pub const ALL: [Category; 11] = [
        Category::Electrical,
        Category::Routing,
        Category::Smt,
        Category::Mask,
        Category::Plane,
        Category::Testpoint,
        Category::Manufacturing,
        Category::HighSpeed,
        Category::Placement,
        Category::SignalIntegrity,
        Category::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Electrical => "Electrical",
            Category::Routing => "Routing",
            Category::Smt => "SMT",
            Category::Mask => "Mask",
            Category::Plane => "Plane",
            Category::Testpoint => "Testpoint",
            Category::Manufacturing => "Manufacturing",
            Category::HighSpeed => "High Speed",
            Category::Placement => "Placement",
            Category::SignalIntegrity => "Signal Integrity",
            Category::Other => "Other",
        }
    }
}

/// What a field holds, for its editor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unit {
    /// A length, written as Altium does (`8mil`).
    Len,
    /// A plain number.
    Num,
    /// `TRUE` or `FALSE`.
    Bool,
    /// One of a list of words.
    Choice(&'static [&'static str]),
    Text,
}

/// One parameter of a rule kind.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub unit: Unit,
    pub default: &'static str,
}

const fn f(key: &'static str, label: &'static str, unit: Unit, default: &'static str) -> Field {
    Field {
        key,
        label,
        unit,
        default,
    }
}

/// A rule kind: its Altium name, where it is listed, whether it takes two
/// scopes (between objects) or one, and its fields.
#[derive(Clone, Copy, Debug)]
pub struct KindInfo {
    pub kind: &'static str,
    pub label: &'static str,
    pub category: Category,
    /// Two scopes: the rule is between objects that the first and the
    /// second take (clearances).
    pub binary: bool,
    pub fields: &'static [Field],
    /// Fields kept per copper layer (`TOPLAYER_PREFWIDTH`...): the
    /// suffixes and their labels.
    pub per_layer: &'static [(&'static str, &'static str)],
    /// What Trace does with it.
    pub used_by: &'static str,
    /// The record kind number Altium keeps before each rule in Rules6,
    /// where known.
    pub altium_id: Option<u16>,
    pub tip: &'static str,
}

use Unit::*;

const NETSCOPE: &[&str] = &["DifferentNets", "SameNet", "AnyNet"];
const CONNECT: &[&str] = &["Relief", "Direct", "NoConnect"];

/// Every rule kind the editor knows, in Altium's order within each category.
pub static KINDS: &[KindInfo] = &[
    // --- Electrical -------------------------------------------------------
    KindInfo {
        kind: "Clearance",
        label: "Clearance",
        category: Category::Electrical,
        binary: true,
        fields: &[
            f("GAP", "Minimum clearance", Len, "8mil"),
            f("NETSCOPE", "Applies to", Choice(NETSCOPE), "DifferentNets"),
            f(
                "IGNOREPADTOPADCLEARANCEINFOOTPRINT",
                "Ignore pad to pad clearance within a footprint",
                Bool,
                "FALSE",
            ),
            f(
                "OBJECTCLEARANCES",
                "Object clearances (Altium table)",
                Text,
                "",
            ),
        ],
        per_layer: &[],
        used_by: "DRC, interactive router, autorouter, pours",
        altium_id: Some(0),
        tip: "Smallest gap between copper of the objects the two scopes take",
    },
    KindInfo {
        kind: "ShortCircuit",
        label: "Short-Circuit",
        category: Category::Electrical,
        binary: true,
        fields: &[f("ALLOWED", "Allow short circuit", Bool, "FALSE")],
        per_layer: &[],
        used_by: "DRC",
        altium_id: Some(15),
        tip: "Whether copper of different nets may touch",
    },
    KindInfo {
        kind: "UnRoutedNet",
        label: "Un-Routed Net",
        category: Category::Electrical,
        binary: false,
        fields: &[f(
            "CHECKBADCONNECTIONS",
            "Check incomplete connections",
            Bool,
            "FALSE",
        )],
        per_layer: &[],
        used_by: "DRC",
        altium_id: Some(16),
        tip: "Nets whose pads are not all joined by copper",
    },
    KindInfo {
        kind: "UnpouredPolygon",
        label: "Modified Polygon",
        category: Category::Electrical,
        binary: false,
        fields: &[],
        per_layer: &[],
        used_by: "kept for Altium",
        altium_id: Some(62),
        tip: "Pours changed since they were last poured",
    },
    // --- Routing ------------------------------------------------------------
    KindInfo {
        kind: "Width",
        label: "Width",
        category: Category::Routing,
        binary: false,
        fields: &[
            f("MINLIMIT", "Min width", Len, "6mil"),
            f("PREFEREDWIDTH", "Preferred width", Len, "8mil"),
            f("MAXLIMIT", "Max width", Len, "50mil"),
            f("IMPEDANCEPROFILEDRIVEN", "Impedance driven", Bool, "FALSE"),
            f("FAVIMP", "Preferred impedance (ohm)", Num, "50"),
        ],
        per_layer: &[
            ("MINWIDTH", "Min"),
            ("PREFWIDTH", "Preferred"),
            ("MAXWIDTH", "Max"),
        ],
        used_by: "DRC, interactive router (track width at route start), autorouter",
        altium_id: Some(2),
        tip: "Track widths the scope's nets may use",
    },
    KindInfo {
        kind: "RoutingTopology",
        label: "Routing Topology",
        category: Category::Routing,
        binary: false,
        fields: &[f(
            "TOPOLOGY",
            "Topology",
            Choice(&[
                "Shortest",
                "Horizontal",
                "Vertical",
                "DaisyChain-Simple",
                "DaisyChain-MidDriven",
                "DaisyChain-Balanced",
                "Starburst",
            ]),
            "Shortest",
        )],
        per_layer: &[],
        used_by: "kept for Altium",
        altium_id: Some(7),
        tip: "The order connections of a net are drawn and routed in",
    },
    KindInfo {
        kind: "RoutingPriority",
        label: "Routing Priority",
        category: Category::Routing,
        binary: false,
        fields: &[f("ROUTINGPRIORITY", "Routing priority (0-100)", Num, "0")],
        per_layer: &[],
        used_by: "kept for Altium",
        altium_id: Some(8),
        tip: "Which nets the autorouter takes first",
    },
    KindInfo {
        kind: "RoutingLayers",
        label: "Routing Layers",
        category: Category::Routing,
        binary: false,
        fields: &[],
        per_layer: &[],
        used_by: "kept for Altium",
        altium_id: Some(9),
        tip: "Copper layers the scope's nets may be routed on",
    },
    KindInfo {
        kind: "RoutingCorners",
        label: "Routing Corners",
        category: Category::Routing,
        binary: false,
        fields: &[
            f(
                "CORNERSTYLE",
                "Style",
                Choice(&["45-Degree", "90-Degree", "Rounded"]),
                "45-Degree",
            ),
            f("MINSETBACK", "Min setback", Len, "100mil"),
            f("MAXSETBACK", "Max setback", Len, "100mil"),
        ],
        per_layer: &[],
        used_by: "kept for Altium",
        altium_id: Some(10),
        tip: "Corner style for the autorouter",
    },
    KindInfo {
        kind: "RoutingVias",
        label: "Routing Via Style",
        category: Category::Routing,
        binary: false,
        fields: &[
            f(
                "VIASTYLE",
                "Via style",
                Choice(&[
                    "Through Hole",
                    "Blind Buried [Adjacent Layers]",
                    "Blind Buried [Any]",
                ]),
                "Through Hole",
            ),
            f("MINWIDTH", "Min via diameter", Len, "20mil"),
            f("WIDTH", "Preferred via diameter", Len, "20mil"),
            f("MAXWIDTH", "Max via diameter", Len, "50mil"),
            f("MINHOLEWIDTH", "Min hole", Len, "10mil"),
            f("HOLEWIDTH", "Preferred hole", Len, "10mil"),
            f("MAXHOLEWIDTH", "Max hole", Len, "28mil"),
        ],
        per_layer: &[],
        used_by: "interactive router (via placed at a layer change)",
        altium_id: Some(11),
        tip: "Via size the router places for the scope's nets",
    },
    KindInfo {
        kind: "FanoutControl",
        label: "Fanout Control",
        category: Category::Routing,
        binary: false,
        fields: &[
            f(
                "FANOUTSTYLE",
                "Fanout style",
                Choice(&["Auto", "InlineRows", "StaggeredRows", "BGA", "UnderPads"]),
                "Auto",
            ),
            f(
                "FANOUTDIRECTION",
                "Direction",
                Choice(&[
                    "Disable",
                    "InOnly",
                    "OutOnly",
                    "InThenOut",
                    "OutThenIn",
                    "Alternating",
                ]),
                "Alternating",
            ),
            f(
                "BGADIR",
                "BGA direction",
                Choice(&[
                    "Out",
                    "In",
                    "TowardsCenter",
                    "AwayFromCenter",
                    "NorthEast",
                    "SouthEast",
                    "SouthWest",
                    "NorthWest",
                ]),
                "Out",
            ),
            f(
                "BGAVIAMODE",
                "BGA via placement",
                Choice(&["CloseToPad", "Centered"]),
                "Centered",
            ),
        ],
        per_layer: &[],
        used_by: "kept for Altium",
        altium_id: Some(49),
        tip: "How the fanout tool leaves part pads",
    },
    KindInfo {
        kind: "DiffPairsRouting",
        label: "Differential Pairs Routing",
        category: Category::Routing,
        binary: false,
        fields: &[
            f("MINLIMIT", "Min gap", Len, "5mil"),
            f("MOSTFREQGAP", "Preferred gap", Len, "6mil"),
            f("MAXLIMIT", "Max gap", Len, "10mil"),
            f("MAXUNCOUPLEDLENGTH", "Max uncoupled length", Len, "500mil"),
            f("IMPEDANCEPROFILEDRIVEN", "Impedance driven", Bool, "FALSE"),
            f(
                "IMPEDANCEPROFILEVALUE",
                "Differential impedance (ohm)",
                Num,
                "100",
            ),
        ],
        per_layer: &[
            ("MINWIDTH", "Min width"),
            ("PREFWIDTH", "Pref width"),
            ("MAXWIDTH", "Max width"),
            ("MINGAP", "Min gap"),
            ("PREFGAP", "Pref gap"),
            ("MAXGAP", "Max gap"),
        ],
        used_by: "interactive router (pair width and gap), DRC",
        altium_id: Some(51),
        tip: "Width and gap pairs are routed at, per layer",
    },
    // --- SMT ------------------------------------------------------------------
    KindInfo {
        kind: "SMDToCorner",
        label: "SMD To Corner",
        category: Category::Smt,
        binary: false,
        fields: &[f("DISTANCE", "Distance", Len, "0mil")],
        per_layer: &[],
        used_by: "kept for Altium",
        altium_id: None,
        tip: "Track run from an SMD pad before its first corner",
    },
    KindInfo {
        kind: "SMDToPlane",
        label: "SMD To Plane",
        category: Category::Smt,
        binary: false,
        fields: &[f("DISTANCE", "Distance", Len, "0mil")],
        per_layer: &[],
        used_by: "kept for Altium",
        altium_id: None,
        tip: "Route length from an SMD pad to a plane via",
    },
    KindInfo {
        kind: "SMDNeckDown",
        label: "SMD Neck-Down",
        category: Category::Smt,
        binary: false,
        fields: &[f("PERCENT", "Neck-down (%)", Num, "50")],
        per_layer: &[],
        used_by: "kept for Altium",
        altium_id: None,
        tip: "Track width leaving a pad as a share of the pad's width",
    },
    // --- Mask -----------------------------------------------------------------
    KindInfo {
        kind: "SolderMaskExpansion",
        label: "Solder Mask Expansion",
        category: Category::Mask,
        binary: false,
        fields: &[
            f("EXPANSION", "Expansion", Len, "4mil"),
            f("ISTENTINGTOP", "Tented top", Bool, "FALSE"),
            f("ISTENTINGBOTTOM", "Tented bottom", Bool, "FALSE"),
            f(
                "SOLDERMASKFROMHOLE",
                "Expand from the hole edge",
                Bool,
                "FALSE",
            ),
        ],
        per_layer: &[],
        used_by: "fab outputs, mask DRC",
        altium_id: Some(13),
        tip: "How far the solder mask opening grows past the pad",
    },
    KindInfo {
        kind: "PasteMaskExpansion",
        label: "Paste Mask Expansion",
        category: Category::Mask,
        binary: false,
        fields: &[f("EXPANSION", "Expansion", Len, "0mil")],
        per_layer: &[],
        used_by: "fab outputs",
        altium_id: Some(14),
        tip: "How far the paste opening grows past the pad (negative shrinks)",
    },
    // --- Plane ------------------------------------------------------------------
    KindInfo {
        kind: "PlaneConnect",
        label: "Power Plane Connect Style",
        category: Category::Plane,
        binary: false,
        fields: &[
            f(
                "PLANECONNECTSTYLE",
                "Connect style",
                Choice(CONNECT),
                "Relief",
            ),
            f("RELIEFEXPANSION", "Expansion", Len, "20mil"),
            f("RELIEFAIRGAP", "Air gap", Len, "10mil"),
            f("RELIEFCONDUCTORWIDTH", "Conductor width", Len, "10mil"),
            f("RELIEFENTRIES", "Entries", Choice(&["2", "4"]), "4"),
        ],
        per_layer: &[],
        used_by: "pours, Gerber planes",
        altium_id: Some(6),
        tip: "How pads and vias join a plane layer",
    },
    KindInfo {
        kind: "PlaneClearance",
        label: "Power Plane Clearance",
        category: Category::Plane,
        binary: false,
        fields: &[f("CLEARANCE", "Clearance", Len, "20mil")],
        per_layer: &[],
        used_by: "pours, Gerber planes",
        altium_id: Some(12),
        tip: "Gap round holes of other nets through a plane layer",
    },
    KindInfo {
        kind: "PolygonConnect",
        label: "Polygon Connect Style",
        category: Category::Plane,
        binary: true,
        fields: &[
            f("CONNECTSTYLE", "Connect style", Choice(CONNECT), "Relief"),
            f("RELIEFCONDUCTORWIDTH", "Conductor width", Len, "10mil"),
            f("AIRGAPWIDTH", "Air gap", Len, "10mil"),
            f("RELIEFENTRIES", "Entries", Choice(&["2", "4"]), "4"),
            f(
                "POLYGONRELIEFANGLE",
                "Angle",
                Choice(&["90 Angle", "45 Angle"]),
                "90 Angle",
            ),
            f(
                "THPAD.CONNECTSTYLE",
                "TH pad: connect style",
                Choice(CONNECT),
                "Relief",
            ),
            f(
                "THPAD.RELIEFCONDUCTORWIDTH",
                "TH pad: conductor width",
                Len,
                "10mil",
            ),
            f("THPAD.AIRGAPWIDTH", "TH pad: air gap", Len, "10mil"),
            f(
                "SMDPAD.CONNECTSTYLE",
                "SMD pad: connect style",
                Choice(CONNECT),
                "Relief",
            ),
            f(
                "SMDPAD.RELIEFCONDUCTORWIDTH",
                "SMD pad: conductor width",
                Len,
                "10mil",
            ),
            f("SMDPAD.AIRGAPWIDTH", "SMD pad: air gap", Len, "10mil"),
            f(
                "VIA.CONNECTSTYLE",
                "Via: connect style",
                Choice(CONNECT),
                "Direct",
            ),
        ],
        per_layer: &[],
        used_by: "pours (re-poured after a change)",
        altium_id: Some(20),
        tip: "How pads and vias join a copper pour of their net",
    },
    // --- Testpoint --------------------------------------------------------------
    KindInfo {
        kind: "FabricationTestpoint",
        label: "Fabrication Testpoint Style",
        category: Category::Testpoint,
        binary: false,
        fields: &[
            f("MINSIZE", "Min size", Len, "40mil"),
            f("PREFEREDSIZE", "Preferred size", Len, "40mil"),
            f("MAXSIZE", "Max size", Len, "100mil"),
            f("ALLOWSIDETOP", "Allow top", Bool, "TRUE"),
            f("ALLOWSIDEBOTTOM", "Allow bottom", Bool, "TRUE"),
        ],
        per_layer: &[],
        used_by: "test point tools",
        altium_id: Some(43),
        tip: "Pads the bare-board tester may probe",
    },
    KindInfo {
        kind: "FabricationTestPointUsage",
        label: "Fabrication Testpoint Usage",
        category: Category::Testpoint,
        binary: false,
        fields: &[
            f("VALID", "Testpoint (Altium code)", Text, "0"),
            f(
                "ALLOWMULTIPLE",
                "Allow more than one per net",
                Bool,
                "FALSE",
            ),
        ],
        per_layer: &[],
        used_by: "test point tools",
        altium_id: Some(44),
        tip: "Which nets need a test point",
    },
    KindInfo {
        kind: "AssemblyTestpoint",
        label: "Assembly Testpoint Style",
        category: Category::Testpoint,
        binary: false,
        fields: &[
            f("MINSIZE", "Min size", Len, "40mil"),
            f("PREFEREDSIZE", "Preferred size", Len, "40mil"),
            f("MAXSIZE", "Max size", Len, "100mil"),
        ],
        per_layer: &[],
        used_by: "test point tools",
        altium_id: Some(57),
        tip: "Pads the in-circuit tester may probe",
    },
    KindInfo {
        kind: "AssemblyTestPointUsage",
        label: "Assembly Testpoint Usage",
        category: Category::Testpoint,
        binary: false,
        fields: &[],
        per_layer: &[],
        used_by: "test point tools",
        altium_id: Some(58),
        tip: "Which nets need an assembly test point",
    },
    // --- Manufacturing ------------------------------------------------------------
    KindInfo {
        kind: "MinimumAnnularRing",
        label: "Minimum Annular Ring",
        category: Category::Manufacturing,
        binary: false,
        fields: &[f("MINIMUMRING", "Minimum ring", Len, "4mil")],
        per_layer: &[],
        used_by: "DRC (fab)",
        altium_id: Some(19),
        tip: "Copper left round a drilled hole",
    },
    KindInfo {
        kind: "AcuteAngle",
        label: "Acute Angle",
        category: Category::Manufacturing,
        binary: false,
        fields: &[f("MINIMUM", "Minimum angle (deg)", Num, "90")],
        per_layer: &[],
        used_by: "kept for Altium",
        altium_id: None,
        tip: "Sharpest corner tracks may make",
    },
    KindInfo {
        kind: "HoleSize",
        label: "Hole Size",
        category: Category::Manufacturing,
        binary: false,
        fields: &[
            f("MINLIMIT", "Min hole", Len, "1mil"),
            f("MAXLIMIT", "Max hole", Len, "100mil"),
            f("ABSOLUTEVALUES", "Absolute values", Bool, "TRUE"),
        ],
        per_layer: &[],
        used_by: "DRC (fab)",
        altium_id: Some(42),
        tip: "Drill sizes the fab can make",
    },
    KindInfo {
        kind: "LayerPairs",
        label: "Layer Pairs",
        category: Category::Manufacturing,
        binary: false,
        fields: &[f("ENFORCE", "Enforce layer pairs", Bool, "TRUE")],
        per_layer: &[],
        used_by: "kept for Altium",
        altium_id: Some(48),
        tip: "Vias may only span the drill pairs of the stack-up",
    },
    KindInfo {
        kind: "HoleToHoleClearance",
        label: "Hole To Hole Clearance",
        category: Category::Manufacturing,
        binary: true,
        fields: &[
            f("GAP", "Hole to hole clearance", Len, "10mil"),
            f(
                "ALLOWSTACKEDMICROVIAS",
                "Allow stacked micro vias",
                Bool,
                "FALSE",
            ),
        ],
        per_layer: &[],
        used_by: "DRC (fab)",
        altium_id: Some(52),
        tip: "Gap between the edges of drilled holes",
    },
    KindInfo {
        kind: "MinimumSolderMaskSliver",
        label: "Minimum Solder Mask Sliver",
        category: Category::Manufacturing,
        binary: true,
        fields: &[f("MINSOLDERMASKWIDTH", "Minimum sliver", Len, "4mil")],
        per_layer: &[],
        used_by: "DRC (fab)",
        altium_id: Some(53),
        tip: "Thinnest web of mask left between two openings",
    },
    KindInfo {
        kind: "SilkToSolderMaskClearance",
        label: "Silk To Solder Mask Clearance",
        category: Category::Manufacturing,
        binary: true,
        fields: &[
            f("MINSILKSCREENTOMASKGAP", "Clearance", Len, "4mil"),
            f(
                "CLEARANCETOEXPOSEDCOPPER",
                "To exposed copper",
                Bool,
                "FALSE",
            ),
        ],
        per_layer: &[],
        used_by: "DRC (fab)",
        altium_id: Some(54),
        tip: "Gap from silkscreen to mask openings",
    },
    KindInfo {
        kind: "SilkToSilkClearance",
        label: "Silk To Silk Clearance",
        category: Category::Manufacturing,
        binary: true,
        fields: &[f("SILKTOSILKCLEARANCE", "Clearance", Len, "5mil")],
        per_layer: &[],
        used_by: "DRC (fab)",
        altium_id: Some(55),
        tip: "Gap between silkscreen of different parts",
    },
    KindInfo {
        kind: "NetAntennae",
        label: "Net Antennae",
        category: Category::Manufacturing,
        binary: false,
        fields: &[f("NETANTENNAETOLERANCE", "Tolerance", Len, "0mil")],
        per_layer: &[],
        used_by: "DRC",
        altium_id: Some(56),
        tip: "Track ends that lead nowhere",
    },
    KindInfo {
        kind: "SilkToBoardRegionClearance",
        label: "Silk To Board Region Clearance",
        category: Category::Manufacturing,
        binary: false,
        fields: &[],
        per_layer: &[],
        used_by: "kept for Altium",
        altium_id: Some(59),
        tip: "Silkscreen kept off board cut-outs",
    },
    KindInfo {
        kind: "BoardOutlineClearance",
        label: "Board Outline Clearance",
        category: Category::Manufacturing,
        binary: false,
        fields: &[f("GAP", "Clearance", Len, "10mil")],
        per_layer: &[],
        used_by: "DRC (copper to edge), stitching",
        altium_id: Some(63),
        tip: "Gap from copper to the board edge",
    },
    KindInfo {
        kind: "BackDrilling",
        label: "Back Drilling",
        category: Category::Manufacturing,
        binary: false,
        fields: &[f("MAXSTUBLENGTH", "Max stub length", Len, "10mil")],
        per_layer: &[],
        used_by: "backdrill tool, via stub DRC",
        altium_id: None,
        tip: "Longest via stub left before back drilling",
    },
    // --- High Speed ------------------------------------------------------------
    KindInfo {
        kind: "ParallelSegment",
        label: "Parallel Segment",
        category: Category::HighSpeed,
        binary: true,
        fields: &[
            f("PARALLELGAP", "Gap", Len, "10mil"),
            f("PARALLELLIMIT", "Parallel limit", Len, "10000mil"),
        ],
        per_layer: &[],
        used_by: "crosstalk DRC",
        altium_id: None,
        tip: "Longest run two nets may keep side by side at the gap",
    },
    KindInfo {
        kind: "Length",
        label: "Length",
        category: Category::HighSpeed,
        binary: false,
        fields: &[
            f("MINLIMIT", "Min length", Len, "0mil"),
            f("MAXLIMIT", "Max length", Len, "100000mil"),
        ],
        per_layer: &[],
        used_by: "Constraint Manager, length tuning, DRC",
        altium_id: None,
        tip: "Routed length the scope's nets must keep within",
    },
    KindInfo {
        kind: "MatchedLengths",
        label: "Matched Lengths",
        category: Category::HighSpeed,
        binary: false,
        fields: &[
            f("TOLERANCE", "Tolerance", Len, "50mil"),
            f(
                "CHECKNETSINDIFFPAIR",
                "Within differential pairs",
                Bool,
                "TRUE",
            ),
            f(
                "CHECKDIFFPAIRVSDIFFPAIR",
                "Pair against pair",
                Bool,
                "FALSE",
            ),
            f("CHECKOTHERS", "All nets of the scope", Bool, "FALSE"),
        ],
        per_layer: &[],
        used_by: "Constraint Manager, length tuning, DRC",
        altium_id: Some(4),
        tip: "Nets of the scope matched in length within the tolerance",
    },
    KindInfo {
        kind: "DaisyChainStubLength",
        label: "Daisy Chain Stub Length",
        category: Category::HighSpeed,
        binary: false,
        fields: &[f("LIMIT", "Max stub length", Len, "1000mil")],
        per_layer: &[],
        used_by: "kept for Altium",
        altium_id: None,
        tip: "Longest branch off a daisy chain",
    },
    KindInfo {
        kind: "ViasUnderSMD",
        label: "Vias Under SMD",
        category: Category::HighSpeed,
        binary: false,
        fields: &[f("ALLOWED", "Allowed", Bool, "FALSE")],
        per_layer: &[],
        used_by: "kept for Altium",
        altium_id: None,
        tip: "Vias in SMD pads",
    },
    KindInfo {
        kind: "MaximumViaCount",
        label: "Maximum Via Count",
        category: Category::HighSpeed,
        binary: false,
        fields: &[f("LIMIT", "Max vias", Num, "1000")],
        per_layer: &[],
        used_by: "Constraint Manager (via count)",
        altium_id: None,
        tip: "Most vias a net may use",
    },
    KindInfo {
        kind: "MaxMinImpedance",
        label: "Impedance",
        category: Category::SignalIntegrity,
        binary: false,
        fields: &[
            f("MINIMUM", "Min impedance (ohm)", Num, "45"),
            f("MAXIMUM", "Max impedance (ohm)", Num, "55"),
        ],
        per_layer: &[],
        used_by: "impedance check",
        altium_id: None,
        tip: "Characteristic impedance the scope's nets must keep within",
    },
    // --- Placement ---------------------------------------------------------------
    KindInfo {
        kind: "RoomDefinition",
        label: "Room Definition",
        category: Category::Placement,
        binary: false,
        fields: &[f(
            "CONFINEMENTSTYLE",
            "Keep objects",
            Choice(&["ConfineIn", "ConfineOut"]),
            "ConfineIn",
        )],
        per_layer: &[],
        used_by: "rooms in rule scopes (WithinRoom, TouchesRoom)",
        altium_id: Some(22),
        tip: "An area of the board that rules can name",
    },
    KindInfo {
        kind: "ComponentClearance",
        label: "Component Clearance",
        category: Category::Placement,
        binary: true,
        fields: &[
            f("GAP", "Horizontal clearance", Len, "10mil"),
            f("VERTICALGAP", "Vertical clearance", Len, "10mil"),
            f("COLLISIONCHECKMODE", "Check mode (Altium code)", Text, "3"),
            f(
                "DONOTCHECKWITHOUT3DBODY",
                "Skip parts without a 3D body",
                Bool,
                "FALSE",
            ),
        ],
        per_layer: &[],
        used_by: "3D clearance check",
        altium_id: Some(24),
        tip: "Gap between part bodies",
    },
    KindInfo {
        kind: "Height",
        label: "Height",
        category: Category::Placement,
        binary: false,
        fields: &[
            f("MINHEIGHT", "Min height", Len, "0mil"),
            f("PREFHEIGHT", "Preferred height", Len, "500mil"),
            f("MAXHEIGHT", "Max height", Len, "1000mil"),
        ],
        per_layer: &[],
        used_by: "3D clearance check",
        altium_id: Some(50),
        tip: "How tall parts in the scope may be",
    },
    // --- Other -------------------------------------------------------------------
    KindInfo {
        kind: "SupplyNets",
        label: "Supply Nets",
        category: Category::SignalIntegrity,
        binary: false,
        fields: &[f("VOLTAGE", "Voltage", Num, "0")],
        per_layer: &[],
        used_by: "PDN analysis",
        altium_id: Some(41),
        tip: "Nets that carry a supply, with its voltage",
    },
];

/// What Trace knows of a rule kind (compared without case).
pub fn info(kind: &str) -> Option<&'static KindInfo> {
    KINDS.iter().find(|k| k.kind.eq_ignore_ascii_case(kind))
}

/// Where a kind is listed; kinds Trace does not know go under Other.
pub fn category(kind: &str) -> Category {
    info(kind).map_or(Category::Other, |k| k.category)
}

/// A parameter of a rule, compared without case.
pub fn param<'a>(r: &'a Rule, key: &str) -> Option<&'a str> {
    r.params
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

/// Sets a parameter, keeping the key's spelling where the rule has it.
pub fn set_param(r: &mut Rule, key: &str, value: impl Into<String>) {
    let k = r
        .params
        .keys()
        .find(|k| k.eq_ignore_ascii_case(key))
        .cloned()
        .unwrap_or_else(|| key.to_string());
    r.params.insert(k, value.into());
}

/// A length parameter in mil.
pub fn mil(r: &Rule, key: &str) -> Option<f64> {
    param(r, key).and_then(Length::parse).map(Length::mil)
}

/// Altium writes lengths in mil with up to four decimals.
pub fn fmt_mil(v: f64) -> String {
    let s = format!("{:.4}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{s}mil")
}

/// Altium's name for copper layer `i` of the board in per-layer rule keys:
/// `TOPLAYER`, `MIDLAYER1`..`MIDLAYER30`, `BOTTOMLAYER`.
pub fn layer_key(board: &Board, i: usize) -> String {
    let copper: Vec<_> = board.stackup.copper_layers().collect();
    let n = copper.len();
    if let Some(id) = copper.get(i).and_then(|l| l.altium_id) {
        return match id {
            1 => "TOPLAYER".into(),
            32 => "BOTTOMLAYER".into(),
            k => format!("MIDLAYER{}", k - 1),
        };
    }
    if i == 0 {
        "TOPLAYER".into()
    } else if i + 1 >= n {
        "BOTTOMLAYER".into()
    } else {
        format!("MIDLAYER{i}")
    }
}

/// A per-layer length (`TOPLAYER_PREFWIDTH`), mil.
pub fn layer_mil(board: &Board, r: &Rule, layer: usize, suffix: &str) -> Option<f64> {
    mil(r, &format!("{}_{}", layer_key(board, layer), suffix))
}

// --- editing --------------------------------------------------------------

/// A name no rule of the board has: `base`, else `base_1`, `base_2`...
pub fn unique_name(rules: &[Rule], base: &str) -> String {
    let taken = |n: &str| rules.iter().any(|r| r.name.eq_ignore_ascii_case(n));
    if !taken(base) {
        return base.to_string();
    }
    (1..)
        .map(|i| format!("{base}_{i}"))
        .find(|n| !taken(n))
        .unwrap()
}

/// A new rule of `kind` for everything, with the kind's defaults, at the
/// lowest priority of its kind so the rules there keep winning.
pub fn new_rule(rules: &[Rule], kind: &str) -> Rule {
    let info = info(kind);
    let kind = info.map_or(kind, |k| k.kind).to_string();
    let mut params = BTreeMap::new();
    let netscope = if info.is_some_and(|k| k.binary) {
        "DifferentNets"
    } else {
        "AnyNet"
    };
    params.insert("NETSCOPE".into(), netscope.into());
    params.insert("LAYERKIND".into(), "SameLayer".into());
    for fd in info.map_or(&[][..], |k| k.fields) {
        params.insert(fd.key.into(), fd.default.into());
    }
    if kind.eq_ignore_ascii_case("Clearance") {
        params.insert("GENERICCLEARANCE".into(), "8mil".into());
    }
    let priority = of_kind(rules, &kind).len() as i64 + 1;
    Rule {
        name: unique_name(rules, &kind),
        kind,
        enabled: true,
        priority,
        scope: vec!["All".into(), "All".into()],
        params,
    }
}

/// The rules of `kind`, highest priority first (index into `rules`).
pub fn of_kind(rules: &[Rule], kind: &str) -> Vec<usize> {
    let mut v: Vec<usize> = (0..rules.len())
        .filter(|&i| rules[i].kind.eq_ignore_ascii_case(kind))
        .collect();
    v.sort_by_key(|&i| (rules[i].priority, i));
    v
}

/// Numbers the rules of `kind` 1, 2, 3... in their present order.
pub fn renumber(rules: &mut [Rule], kind: &str) {
    for (p, i) in of_kind(rules, kind).into_iter().enumerate() {
        rules[i].priority = p as i64 + 1;
    }
}

/// Moves rule `i` one place up (`up`) or down in its kind's priorities;
/// false when it is already at that end.
pub fn shift(rules: &mut [Rule], i: usize, up: bool) -> bool {
    let Some(kind) = rules.get(i).map(|r| r.kind.clone()) else {
        return false;
    };
    renumber(rules, &kind);
    let order = of_kind(rules, &kind);
    let k = order.iter().position(|&j| j == i).unwrap();
    let other = if up {
        if k == 0 {
            return false;
        }
        order[k - 1]
    } else {
        if k + 1 >= order.len() {
            return false;
        }
        order[k + 1]
    };
    let p = rules[i].priority;
    rules[i].priority = rules[other].priority;
    rules[other].priority = p;
    true
}

/// A copy of rule `i` named afresh, just below it in priority.
pub fn duplicate(rules: &mut Vec<Rule>, i: usize) -> Option<usize> {
    let mut r = rules.get(i)?.clone();
    r.name = unique_name(rules, &format!("{}_copy", r.name));
    for x in rules.iter_mut() {
        if x.kind.eq_ignore_ascii_case(&r.kind) && x.priority > r.priority {
            x.priority += 1;
        }
    }
    r.priority += 1;
    rules.push(r);
    Some(rules.len() - 1)
}

/// What is wrong with a set of rules: names used twice, priorities
/// shared within a kind, lengths that do not read, min above max.
pub fn problems(rules: &[Rule]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (i, r) in rules.iter().enumerate() {
        if r.name.trim().is_empty() {
            out.push((i, "has no name".into()));
        }
        if rules[..i]
            .iter()
            .any(|o| o.name.eq_ignore_ascii_case(&r.name) && o.kind.eq_ignore_ascii_case(&r.kind))
        {
            out.push((i, format!("name {} is used twice", r.name)));
        }
        if rules[..i]
            .iter()
            .any(|o| o.kind.eq_ignore_ascii_case(&r.kind) && o.priority == r.priority)
        {
            out.push((
                i,
                format!(
                    "shares priority {} with another {} rule",
                    r.priority, r.kind
                ),
            ));
        }
        if let Some(k) = info(&r.kind) {
            for fd in k.fields {
                if fd.unit == Unit::Len {
                    if let Some(v) = param(r, fd.key) {
                        if !v.trim().is_empty() && Length::parse(v).is_none() {
                            out.push((i, format!("{}: '{v}' is not a length", fd.label)));
                        }
                    }
                }
            }
        }
        for (lo, hi) in [
            ("MINLIMIT", "MAXLIMIT"),
            ("MINWIDTH", "MAXWIDTH"),
            ("MINHOLEWIDTH", "MAXHOLEWIDTH"),
            ("MINHEIGHT", "MAXHEIGHT"),
        ] {
            if let (Some(a), Some(b)) = (mil(r, lo), mil(r, hi)) {
                if a > b + 1e-6 {
                    out.push((i, format!("min {} is above max {}", fmt_mil(a), fmt_mil(b))));
                }
            }
        }
        for (k, s) in r.scope.iter().enumerate() {
            if let Err(e) = check_scope(s) {
                out.push((i, format!("scope {}: {e}", k + 1)));
            }
        }
    }
    out
}

/// A starting set for a board that has no rules (as from Allegro or
/// Gerbers): Altium's defaults, sized from the board's own copper where it
/// tells (its narrowest track and its most common via).
pub fn defaults(board: &Board) -> Vec<Rule> {
    let g = &board.geometry;
    let mut widths: Vec<f64> = g
        .tracks
        .iter()
        .filter(|t| t.net.is_some() && t.component.is_none())
        .map(|t| t.width.mil())
        .filter(|&w| w > 0.5)
        .collect();
    widths.sort_by(f64::total_cmp);
    let min_w = widths.first().copied().unwrap_or(6.0).max(2.0);
    let pref_w = widths
        .get(widths.len() / 2)
        .copied()
        .unwrap_or(8.0)
        .max(min_w);
    let max_w = widths.last().copied().unwrap_or(50.0).max(pref_w);
    let mut vias: BTreeMap<(i64, i64), usize> = BTreeMap::new();
    for v in &g.vias {
        let k = (
            (v.diameter.mil() * 100.0).round() as i64,
            (v.hole.mil() * 100.0).round() as i64,
        );
        *vias.entry(k).or_default() += 1;
    }
    let via = vias
        .iter()
        .max_by_key(|(_, n)| **n)
        .map(|(k, _)| (k.0 as f64 / 100.0, k.1 as f64 / 100.0))
        .unwrap_or((20.0, 10.0));
    let mut out: Vec<Rule> = Vec::new();
    let mut add = |kind: &str, kv: &[(&str, String)]| {
        let mut r = new_rule(&out, kind);
        for (k, v) in kv {
            set_param(&mut r, k, v.clone());
        }
        out.push(r);
    };
    // Dense boards route their narrowest tracks at about their gap.
    let gap = fmt_mil(if widths.is_empty() {
        8.0
    } else {
        min_w.clamp(3.0, 8.0)
    });
    add(
        "Clearance",
        &[("GAP", gap.clone()), ("GENERICCLEARANCE", gap)],
    );
    add("ShortCircuit", &[]);
    add("UnRoutedNet", &[]);
    add(
        "Width",
        &[
            ("MINLIMIT", fmt_mil(min_w)),
            ("PREFEREDWIDTH", fmt_mil(pref_w)),
            ("MAXLIMIT", fmt_mil(max_w)),
        ],
    );
    add(
        "RoutingVias",
        &[
            ("WIDTH", fmt_mil(via.0)),
            ("MINWIDTH", fmt_mil(via.0)),
            ("MAXWIDTH", fmt_mil(via.0.max(50.0))),
            ("HOLEWIDTH", fmt_mil(via.1)),
            ("MINHOLEWIDTH", fmt_mil(via.1)),
            ("MAXHOLEWIDTH", fmt_mil(via.1.max(28.0))),
        ],
    );
    if !board.diff_pairs.is_empty() {
        add("DiffPairsRouting", &[]);
    }
    for k in [
        "SolderMaskExpansion",
        "PasteMaskExpansion",
        "PlaneConnect",
        "PlaneClearance",
        "PolygonConnect",
        "MinimumAnnularRing",
        "HoleSize",
        "HoleToHoleClearance",
        "MinimumSolderMaskSliver",
        "SilkToSolderMaskClearance",
        "SilkToSilkClearance",
        "BoardOutlineClearance",
        "ComponentClearance",
        "Height",
    ] {
        add(k, &[]);
    }
    out
}

// --- scopes ------------------------------------------------------------------

/// The query words Trace judges in rule scopes.
pub const SCOPE_TERMS: &[&str] = &[
    "All",
    "InNet",
    "InNetClass",
    "InDifferentialPair",
    "InDifferentialPairClass",
    "InAnyDifferentialPair",
    "InComponent",
    "InComponentClass",
    "HasFootprint",
    "InAnyComponent",
    "IsFree",
    "IsThruComponent",
    "IsSMTComponent",
    "OnLayer",
    "OnTopLayer",
    "OnBottomLayer",
    "OnCopper",
    "OnSignal",
    "WithinRoom",
    "TouchesRoom",
    "IsTrack",
    "IsArc",
    "IsVia",
    "IsPad",
    "IsSMDPad",
    "IsTHPad",
    "IsRegion",
    "IsFill",
    "IsKeepout",
    "InPolygon",
    "IsPolygon",
    "InPoly",
    "IsPoly",
    "InAnyPolygon",
    "HasFootprintPad",
    "HoleSize",
    "ObjectKind",
    "Name",
    "Identifier",
];

/// How a scope reads: `Ok(words Trace does not judge)` (the rule is then
/// a maybe where they matter), or why it does not parse.
pub fn check_scope(scope: &str) -> Result<Vec<String>, String> {
    let s = scope.trim();
    if s.is_empty() {
        return Err("the scope is empty (use All)".into());
    }
    let e = crate::query::parse_any(s)?;
    let mut unknown = Vec::new();
    fn walk(e: &crate::query::Expr, out: &mut Vec<String>) {
        use crate::query::Expr as E;
        match e {
            E::And(a, b) | E::Or(a, b) => {
                walk(a, out);
                walk(b, out);
            }
            E::Cmp(a, _, _) => walk(a, out),
            E::Not(a) => walk(a, out),
            E::Call(w, _) | E::Word(w) => {
                let known = SCOPE_TERMS.iter().any(|t| t.eq_ignore_ascii_case(w))
                    || ["true", "false", "like"].contains(&w.as_str());
                if !known && !out.iter().any(|x: &String| x.eq_ignore_ascii_case(w)) {
                    out.push(w.clone());
                }
            }
            _ => {}
        }
    }
    walk(&e, &mut unknown);
    Ok(unknown)
}

/// A scope for one net, class, pair or layer, as the editor's quick picks
/// write it.
pub fn scope_of(term: &str, name: &str) -> String {
    format!("{term}('{}')", name.replace('\'', ""))
}

/// Counts of the board objects a scope takes: (nets, tracks, vias, pads),
/// and the tracks, vias and parts themselves for showing on the board.
#[derive(Clone, Debug, Default)]
pub struct Matches {
    pub nets: usize,
    pub tracks: usize,
    pub vias: usize,
    pub pads: usize,
    /// Objects the scope may take, on terms Trace cannot judge.
    pub maybe: usize,
    pub sel: crate::select::Sel,
}

/// What `scope` takes on the board.
pub fn matches(board: &Board, scope: &str) -> Matches {
    let scopes = Scopes::new(board);
    let mut m = Matches::default();
    let layer_of = |l: &crate::LayerRef| match l {
        crate::LayerRef::Copper(i) => Some(*i),
        _ => None,
    };
    for n in 0..board.nets.len() {
        let t = Thing {
            net: Some(n as u32),
            obj: Obj::Track,
            component: None,
            hole: None,
            layer: None,
            bbox: None,
        };
        if scopes.judge(scope, &t) == Some(true) {
            m.nets += 1;
        }
    }
    let g = &board.geometry;
    for (i, t) in g.tracks.iter().enumerate() {
        if t.component.is_some() || t.text {
            continue;
        }
        let Some(layer) = layer_of(&t.layer) else {
            continue;
        };
        let (a, b) = (
            [t.start.x.mil(), t.start.y.mil()],
            [t.end.x.mil(), t.end.y.mil()],
        );
        let th = Thing {
            net: t.net,
            obj: Obj::Track,
            component: None,
            hole: None,
            layer: Some(layer),
            bbox: Some([
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[0].max(b[0]),
                a[1].max(b[1]),
            ]),
        };
        match scopes.judge(scope, &th) {
            Some(true) => {
                m.tracks += 1;
                m.sel.tracks.insert(i);
            }
            None => m.maybe += 1,
            _ => {}
        }
    }
    for (i, v) in g.vias.iter().enumerate() {
        let (x, y, r) = (v.at.x.mil(), v.at.y.mil(), v.diameter.mil() / 2.0);
        let th = Thing {
            net: v.net,
            obj: Obj::Via,
            component: None,
            hole: Some(v.hole.mil()),
            layer: Some(v.from_layer),
            bbox: Some([x - r, y - r, x + r, y + r]),
        };
        match scopes.judge(scope, &th) {
            Some(true) => {
                m.vias += 1;
                m.sel.vias.insert(i);
            }
            None => m.maybe += 1,
            _ => {}
        }
    }
    for p in &g.pads {
        let (x, y) = (p.at.x.mil(), p.at.y.mil());
        let (hx, hy) = (p.size_x.mil() / 2.0, p.size_y.mil() / 2.0);
        let th = Thing {
            net: p.net,
            obj: if p.hole.mil() > 0.0 {
                Obj::ThPad
            } else {
                Obj::SmdPad
            },
            component: p.component,
            hole: (p.hole.mil() > 0.0).then(|| p.hole.mil()),
            layer: layer_of(&p.layer),
            bbox: Some([x - hx, y - hy, x + hx, y + hy]),
        };
        match scopes.judge(scope, &th) {
            Some(true) => {
                m.pads += 1;
                if let Some(c) = p.component {
                    m.sel.parts.insert(c as usize);
                }
            }
            None => m.maybe += 1,
            _ => {}
        }
    }
    m
}

// --- the rule that applies ----------------------------------------------------

/// Sizes the rules give one net, for routing it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetSizes {
    /// (min, preferred, max) track width, mil, and the rule's name.
    pub width: Option<([f64; 3], String)>,
    /// Clearance to other copper, mil, and the rule's name.
    pub clearance: Option<(f64, String)>,
    /// Via (diameter, hole), mil, and the rule's name.
    pub via: Option<((f64, f64), String)>,
}

/// Picks the rule that applies, highest priority first.
pub struct Resolver<'a> {
    board: &'a Board,
    scopes: Scopes,
    by_kind: BTreeMap<String, Vec<usize>>,
}

fn is_all(s: Option<&String>) -> bool {
    s.is_none_or(|s| s.trim().eq_ignore_ascii_case("All"))
}

impl<'a> Resolver<'a> {
    pub fn new(board: &'a Board) -> Resolver<'a> {
        let mut by_kind: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, r) in board.rules.iter().enumerate() {
            if r.enabled {
                by_kind
                    .entry(r.kind.to_ascii_lowercase())
                    .or_default()
                    .push(i);
            }
        }
        for v in by_kind.values_mut() {
            v.sort_by_key(|&i| (board.rules[i].priority, i));
        }
        Resolver {
            board,
            scopes: Scopes::new(board),
            by_kind,
        }
    }

    fn rules(&self, kind: &str) -> impl Iterator<Item = &'a Rule> + '_ {
        self.by_kind
            .get(&kind.to_ascii_lowercase())
            .into_iter()
            .flatten()
            .map(|&i| &self.board.rules[i])
    }

    fn takes(&self, scope: Option<&String>, net: u32, obj: Obj, layer: Option<u8>) -> bool {
        let Some(s) = scope else { return true };
        let t = Thing {
            net: Some(net),
            obj,
            component: None,
            hole: None,
            layer,
            bbox: None,
        };
        self.scopes.judge(s, &t) == Some(true)
    }

    /// The first rule of `kind` by priority whose (first) scope takes the
    /// net's tracks on `layer`.
    pub fn unary(&self, kind: &str, net: u32, layer: Option<u8>) -> Option<&'a Rule> {
        self.rules(kind)
            .find(|r| self.takes(r.scope.first(), net, Obj::Track, layer))
    }

    /// The width rule for a net's tracks on copper layer `layer`.
    pub fn width(&self, net: u32, layer: u8) -> Option<([f64; 3], String)> {
        let r = self.unary("Width", net, Some(layer))?;
        let l = layer as usize;
        let get =
            |suffix: &str, key: &str| layer_mil(self.board, r, l, suffix).or_else(|| mil(r, key));
        let pref = get("PREFWIDTH", "PREFEREDWIDTH")
            .or_else(|| mil(r, "PREFERREDWIDTH"))
            .or_else(|| get("MINWIDTH", "MINLIMIT"))?;
        let min = get("MINWIDTH", "MINLIMIT").unwrap_or(pref);
        let max = get("MAXWIDTH", "MAXLIMIT").unwrap_or(pref);
        Some(([min, pref, max], r.name.clone()))
    }

    /// The clearance a net keeps from copper in general: the first
    /// Clearance rule by priority that takes the net on one side and
    /// everything on the other (or everything on both).
    pub fn clearance(&self, net: u32) -> Option<(f64, String)> {
        self.rules("Clearance")
            .find(|r| {
                let (a, b) = (r.scope.first(), r.scope.get(1));
                (self.takes(a, net, Obj::Track, None) && is_all(b))
                    || (self.takes(b, net, Obj::Track, None) && is_all(a))
            })
            .and_then(|r| Some((mil(r, "GAP")?, r.name.clone())))
    }

    /// The clearance between copper of nets `a` and `b`.
    pub fn clearance_between(&self, a: u32, b: u32) -> Option<(f64, String)> {
        self.rules("Clearance")
            .find(|r| {
                let (s1, s2) = (r.scope.first(), r.scope.get(1));
                (self.takes(s1, a, Obj::Track, None) && self.takes(s2, b, Obj::Track, None))
                    || (self.takes(s1, b, Obj::Track, None) && self.takes(s2, a, Obj::Track, None))
            })
            .and_then(|r| Some((mil(r, "GAP")?, r.name.clone())))
    }

    /// The via (diameter, hole) a net's routing via style asks for.
    pub fn via(&self, net: u32) -> Option<((f64, f64), String)> {
        let r = self
            .rules("RoutingVias")
            .find(|r| self.takes(r.scope.first(), net, Obj::Via, None))?;
        let d = mil(r, "WIDTH").or_else(|| mil(r, "MINWIDTH"))?;
        let h = mil(r, "HOLEWIDTH").or_else(|| mil(r, "MINHOLEWIDTH"))?;
        Some(((d, h), r.name.clone()))
    }

    /// Width and gap for a pair carrying `net` on copper layer `layer`,
    /// from its Differential Pairs Routing rule.
    pub fn pair(&self, net: u32, layer: u8) -> Option<((f64, f64), String)> {
        let r = self.unary("DiffPairsRouting", net, Some(layer))?;
        let l = layer as usize;
        let w = layer_mil(self.board, r, l, "PREFWIDTH")
            .or_else(|| layer_mil(self.board, r, l, "MINWIDTH"))
            .or_else(|| self.width(net, layer).map(|w| w.0[1]))?;
        let g = layer_mil(self.board, r, l, "PREFGAP")
            .or_else(|| mil(r, "MOSTFREQGAP"))
            .or_else(|| mil(r, "MINLIMIT"))?;
        Some(((w, g), r.name.clone()))
    }

    /// Everything the router takes from the rules for one net.
    pub fn sizes(&self, net: u32, layer: u8) -> NetSizes {
        NetSizes {
            width: self.width(net, layer),
            clearance: self.clearance(net),
            via: self.via(net),
        }
    }

    /// The rule of each kind that applies to a net, for showing: (kind,
    /// rule name, what it asks). Binary kinds are taken against everything.
    pub fn report(&self, net: u32) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        let copper = self.board.stackup.copper_layers().count().max(1);
        for (kind, idx) in &self.by_kind {
            let Some(&first) = idx.first() else { continue };
            let k = &self.board.rules[first].kind;
            let binary = info(k).is_some_and(|i| i.binary);
            let hit = idx.iter().map(|&i| &self.board.rules[i]).find(|r| {
                if binary {
                    let (a, b) = (r.scope.first(), r.scope.get(1));
                    (self.takes(a, net, Obj::Track, None) && is_all(b))
                        || (self.takes(b, net, Obj::Track, None) && is_all(a))
                } else {
                    self.takes(r.scope.first(), net, Obj::Track, None)
                }
            });
            let Some(r) = hit else { continue };
            let what = match kind.as_str() {
                "width" => (0..copper)
                    .filter_map(|l| self.width(net, l as u8))
                    .map(|w| fmt_mil(w.0[1]))
                    .fold(Vec::<String>::new(), |mut v, s| {
                        if !v.contains(&s) {
                            v.push(s);
                        }
                        v
                    })
                    .join(" / "),
                "diffpairsrouting" => self
                    .pair(net, 0)
                    .map(|((w, g), _)| format!("{} wide, {} gap (top)", fmt_mil(w), fmt_mil(g)))
                    .unwrap_or_default(),
                "routingvias" => self
                    .via(net)
                    .map(|((d, h), _)| format!("{} / {} hole", fmt_mil(d), fmt_mil(h)))
                    .unwrap_or_default(),
                _ => summary(r),
            };
            out.push((k.clone(), r.name.clone(), what));
        }
        out
    }
}

/// A short line of a rule's main values, for lists.
pub fn summary(r: &Rule) -> String {
    let Some(k) = info(&r.kind) else {
        return String::new();
    };
    k.fields
        .iter()
        .filter(|f| !matches!(f.unit, Unit::Text))
        .take(3)
        .filter_map(|f| param(r, f.key).map(|v| format!("{} {}", short(f.label), v.trim())))
        .collect::<Vec<_>>()
        .join(", ")
}

fn short(label: &str) -> &str {
    label.split(':').next_back().unwrap_or(label).trim()
}

/// The Altium Rules6 record of a rule Trace made, with every key Altium
/// writes for its kind: `None` when the kind's record number is not known.
pub fn altium_record(r: &Rule, unique_id: &str) -> Option<(u16, Vec<(String, String)>)> {
    let id = info(&r.kind)?.altium_id?;
    let mut v: Vec<(String, String)> = [
        ("SELECTION", "FALSE"),
        ("LAYER", "TOP"),
        ("LOCKED", "FALSE"),
        ("POLYGONOUTLINE", "FALSE"),
        ("USERROUTED", "TRUE"),
        ("KEEPOUT", "FALSE"),
        ("UNIONINDEX", "0"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let mut push = |k: &str, val: String| v.push((k.to_string(), val));
    push("RULEKIND", info(&r.kind)?.kind.to_string());
    push(
        "NETSCOPE",
        param(r, "NETSCOPE").unwrap_or("AnyNet").to_string(),
    );
    push(
        "LAYERKIND",
        param(r, "LAYERKIND").unwrap_or("SameLayer").to_string(),
    );
    for k in 0..2 {
        let s = r.scope.get(k).cloned().unwrap_or_else(|| "All".into());
        push(
            if k == 0 {
                "SCOPE1EXPRESSION"
            } else {
                "SCOPE2EXPRESSION"
            },
            s,
        );
    }
    push("NAME", r.name.clone());
    push("ENABLED", if r.enabled { "TRUE" } else { "FALSE" }.into());
    push("PRIORITY", r.priority.to_string());
    push("COMMENT", param(r, "COMMENT").unwrap_or("").to_string());
    push("UNIQUEID", unique_id.to_string());
    push("DEFINEDBYLOGICALDOCUMENT", "FALSE".into());
    for (k, val) in &r.params {
        let up = k.to_ascii_uppercase();
        if ["NETSCOPE", "LAYERKIND", "COMMENT"].contains(&up.as_str()) {
            continue;
        }
        v.push((k.clone(), val.clone()));
    }
    if r.kind.eq_ignore_ascii_case("Clearance") && param(r, "GENERICCLEARANCE").is_none() {
        if let Some(g) = param(r, "GAP") {
            v.push(("GENERICCLEARANCE".into(), g.to_string()));
        }
    }
    Some((id, v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LayerKind, Net, NetClass, StackLayer};

    fn rule(kind: &str, name: &str, pri: i64, scope: [&str; 2], kv: &[(&str, &str)]) -> Rule {
        Rule {
            kind: kind.into(),
            name: name.into(),
            enabled: true,
            priority: pri,
            scope: scope.iter().map(|s| s.to_string()).collect(),
            params: kv
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn board() -> Board {
        let mut b = Board::default();
        for (i, n) in ["Top", "GND", "Bottom"].iter().enumerate() {
            b.stackup.layers.push(StackLayer {
                name: n.to_string(),
                altium_id: Some(if i == 0 {
                    1
                } else if i == 2 {
                    32
                } else {
                    2
                }),
                kind: LayerKind::Copper,
                thickness: Length::from_mil(1.4),
                dielectric_constant: None,
                loss_tangent: None,
                roughness_um: None,
                material: None,
            });
        }
        for n in ["RF_IN", "CLK_P", "CLK_N", "VCC"] {
            b.nets.push(Net {
                name: n.into(),
                ..Default::default()
            });
        }
        b.net_classes.push(NetClass {
            name: "50R".into(),
            members: vec!["RF_IN".into()],
        });
        b.rules = vec![
            rule(
                "Width",
                "Width",
                2,
                ["All", "All"],
                &[
                    ("MINLIMIT", "5mil"),
                    ("PREFEREDWIDTH", "8mil"),
                    ("MAXLIMIT", "20mil"),
                ],
            ),
            rule(
                "Width",
                "RF",
                1,
                ["InNetClass('50R')", "All"],
                &[
                    ("MINLIMIT", "10mil"),
                    ("PREFEREDWIDTH", "12mil"),
                    ("MAXLIMIT", "14mil"),
                    ("BOTTOMLAYER_PREFWIDTH", "13.5mil"),
                ],
            ),
            rule(
                "Clearance",
                "Clearance",
                2,
                ["All", "All"],
                &[("GAP", "6mil")],
            ),
            rule(
                "Clearance",
                "RF_gap",
                1,
                ["All", "InNet('RF_IN')"],
                &[("GAP", "20mil")],
            ),
            rule(
                "RoutingVias",
                "RoutingVias",
                1,
                ["All", "All"],
                &[("WIDTH", "18mil"), ("HOLEWIDTH", "8mil")],
            ),
            rule(
                "DiffPairsRouting",
                "DP",
                1,
                ["All", "All"],
                &[
                    ("MOSTFREQGAP", "6mil"),
                    ("TOPLAYER_PREFWIDTH", "5mil"),
                    ("TOPLAYER_PREFGAP", "5.5mil"),
                ],
            ),
        ];
        b
    }

    #[test]
    fn highest_priority_rule_wins() {
        let b = board();
        let r = Resolver::new(&b);
        let w = r.width(0, 0).unwrap();
        assert_eq!(w.1, "RF");
        assert_eq!(w.0, [10.0, 12.0, 14.0]);
        assert_eq!(
            r.width(0, 2).unwrap().0[1],
            13.5,
            "per-layer preferred width"
        );
        assert_eq!(r.width(3, 0).unwrap().0[1], 8.0);
        assert_eq!(r.clearance(0).unwrap(), (20.0, "RF_gap".into()));
        assert_eq!(r.clearance(3).unwrap().0, 6.0);
        assert_eq!(r.clearance_between(3, 0).unwrap().0, 20.0);
        assert_eq!(r.via(1).unwrap().0, (18.0, 8.0));
        assert_eq!(r.pair(1, 0).unwrap().0, (5.0, 5.5));
        // Bottom: no per-layer keys, the net's width and the pair gap.
        assert_eq!(r.pair(1, 2).unwrap().0, (8.0, 6.0));
    }

    #[test]
    fn disabled_rules_do_not_apply() {
        let mut b = board();
        b.rules[1].enabled = false;
        assert_eq!(Resolver::new(&b).width(0, 0).unwrap().1, "Width");
    }

    #[test]
    fn new_rules_take_defaults_and_lowest_priority() {
        let b = board();
        let r = new_rule(&b.rules, "width");
        assert_eq!(r.kind, "Width");
        assert_eq!(r.name, "Width_1");
        assert_eq!(r.priority, 3);
        assert_eq!(param(&r, "PREFEREDWIDTH"), Some("8mil"));
        assert_eq!(param(&r, "NETSCOPE"), Some("AnyNet"));
        let c = new_rule(&b.rules, "Clearance");
        assert_eq!(param(&c, "NETSCOPE"), Some("DifferentNets"));
    }

    #[test]
    fn priorities_shift_and_renumber() {
        let mut b = board();
        assert!(shift(&mut b.rules, 0, true));
        assert_eq!(b.rules[0].priority, 1);
        assert_eq!(b.rules[1].priority, 2);
        assert!(!shift(&mut b.rules, 0, true));
        let d = duplicate(&mut b.rules, 0).unwrap();
        assert_eq!(b.rules[d].priority, 2);
        assert_eq!(b.rules[1].priority, 3);
        assert_eq!(b.rules[d].name, "Width_copy");
        assert!(problems(&b.rules).is_empty(), "{:?}", problems(&b.rules));
    }

    #[test]
    fn scopes_are_checked() {
        assert_eq!(
            check_scope("InNetClass('50R') and not InNet('X')")
                .unwrap()
                .len(),
            0
        );
        assert_eq!(check_scope("IsBGA").unwrap(), vec!["isbga".to_string()]);
        assert!(check_scope("InNet('A'").is_err());
        assert!(check_scope("").is_err());
        assert!(check_scope("HoleSize > 10mil").unwrap().is_empty());
    }

    #[test]
    fn problems_are_found() {
        let mut b = board();
        b.rules[0].priority = 1;
        set_param(&mut b.rules[2], "gap", "8 furlongs");
        b.rules[3].scope[1] = "InNet('RF_IN'".into();
        let p = problems(&b.rules);
        assert!(
            p.iter().any(|(i, s)| *i == 1 && s.contains("priority")),
            "{p:?}"
        );
        assert!(
            p.iter().any(|(i, s)| *i == 2 && s.contains("not a length")),
            "{p:?}"
        );
        assert!(
            p.iter().any(|(i, s)| *i == 3 && s.contains("scope 2")),
            "{p:?}"
        );
    }

    #[test]
    fn defaults_follow_the_copper() {
        let mut b = board();
        b.rules.clear();
        let d = defaults(&b);
        assert!(problems(&d).is_empty(), "{:?}", problems(&d));
        assert!(d
            .iter()
            .all(|r| info(&r.kind).is_some_and(|k| k.altium_id.is_some())));
        b.rules = d;
        let r = Resolver::new(&b);
        assert_eq!(r.clearance(0).unwrap().0, 8.0);
        assert_eq!(r.via(0).unwrap().0, (20.0, 10.0));
    }

    #[test]
    fn matches_count_nets() {
        let b = board();
        assert_eq!(matches(&b, "InNetClass('50R')").nets, 1);
        assert_eq!(matches(&b, "All").nets, 4);
    }

    #[test]
    fn records_carry_altium_keys() {
        let b = board();
        let (id, kv) = altium_record(&b.rules[2], "ABCDEFGH").unwrap();
        assert_eq!(id, 0);
        let get = |k: &str| kv.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str());
        assert_eq!(get("RULEKIND"), Some("Clearance"));
        assert_eq!(get("GENERICCLEARANCE"), Some("6mil"));
        assert_eq!(get("SCOPE2EXPRESSION"), Some("All"));
        assert_eq!(fmt_mil(7.874), "7.874mil");
        assert_eq!(fmt_mil(8.0), "8mil");
    }
}
