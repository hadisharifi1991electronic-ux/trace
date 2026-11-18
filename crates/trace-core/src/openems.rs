//! openEMS as a second, independent solver for via transitions.
//!
//! The transition Trace's own FDTD solves (stack-up, planes with their
//! antipads, pads, barrels, return vias, traces, the resistors at the
//! traces' ends and the probes on them) is written as an openEMS model:
//! the CSXCAD XML that `openEMS.exe` runs as it is, on the same graded
//! grid (or a finer one) and with the same PML. openEMS (GPL-3.0, by
//! Thorsten Liebig) is run as an external program; its voltage and current
//! probes are read back and turned into S-parameters by the same power-wave
//! extraction Trace uses, so the two results differ only by the solvers.
//!
//! openEMS draws round metal in steps (it has no conformal cells), so the
//! export defaults to half of Trace's smallest cell.

use crate::fdtd::{self, Cx, Model, ViaResult, ViaSetup};
use crate::Stackup;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const MIL: f64 = 25.4e-6;

/// The model's file name inside the run folder.
pub const MODEL: &str = "trace_via.xml";

/// What was written.
#[derive(Clone, Debug)]
pub struct Export {
    pub dir: PathBuf,
    pub xml: PathBuf,
    pub cells: usize,
    /// The smallest cell used, mil.
    pub cell: f64,
    pub notes: Vec<String>,
}

/// Grid index of the line nearest to `p`.
fn near(v: &[f64], p: f64) -> usize {
    let mut best = 0;
    for (i, &q) in v.iter().enumerate() {
        if (q - p).abs() < (v[best] - p).abs() {
            best = i;
        }
    }
    best
}

/// The probes' and ports' grid positions, as the solver places them.
struct Layout {
    ksheet: Vec<usize>,
    x_port: [usize; 2],
    x_probe: [usize; 4],
    /// Per signal via, per trace: the y lines the trace's metal covers.
    rows: Vec<[Vec<usize>; 2]>,
}

fn layout(m: &Model) -> Layout {
    let (x, y, z) = (&m.x, &m.y, &m.z);
    let ksheet = m.st.sheet.iter().map(|&s| near(z, s)).collect();
    let x_port = [near(x, -m.port_at), near(x, m.port_at)];
    let gap = (m.port_at - m.probe_at) / 3.0;
    let x_probe = [
        near(x, -m.probe_at),
        near(x, m.probe_at),
        near(x, -m.probe_at - gap),
        near(x, m.probe_at + gap),
    ];
    let rows = m
        .sig
        .iter()
        .map(|c| {
            let row = |t: usize| -> Vec<usize> {
                let js: Vec<usize> = (0..y.len())
                    .filter(|&j| (y[j] - c[1]).abs() <= m.w[t] / 2.0 - m.edge + m.d0 * 0.01)
                    .collect();
                if js.is_empty() {
                    vec![near(y, c[1])]
                } else {
                    js
                }
            };
            [row(0), row(1)]
        })
        .collect();
    Layout {
        ksheet,
        x_port,
        x_probe,
        rows,
    }
}

/// The stack-up as the solver sees it (materials given on the setup in
/// place of the stack-up's).
fn stackup_for(stackup: &Stackup, setup: &ViaSetup) -> Stackup {
    if setup.er.is_some() || setup.df.is_some() {
        fdtd::materials(stackup, setup.er, setup.df)
    } else {
        stackup.clone()
    }
}

/// The setup openEMS runs: Trace's, on half its smallest cell unless one
/// is given (`cell` > 0, mil).
pub fn setup_for(stackup: &Stackup, setup: &ViaSetup, cell: f64) -> Result<ViaSetup, String> {
    let mut s = setup.clone();
    s.fields.clear();
    if cell > 0.0 {
        s.cell = cell;
    } else {
        let m = fdtd::model(&stackup_for(stackup, setup), setup)?;
        s.cell = (m.d0 / 2.0).max(0.25);
    }
    Ok(s)
}

fn num(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".into()
    } else {
        s.to_string()
    }
}

fn lines(v: &[f64], flip: bool) -> String {
    v.iter()
        .map(|&p| num(if flip { -p } else { p }))
        .collect::<Vec<_>>()
        .join(",")
}

fn pt(tag: &str, x: f64, y: f64, z: f64) -> String {
    format!(r#"<{tag} X="{}" Y="{}" Z="{}"/>"#, num(x), num(y), num(z))
}

fn boxp(prio: u32, a: [f64; 3], b: [f64; 3]) -> String {
    format!(
        r#"<Box Priority="{prio}">{}{}</Box>"#,
        pt("P1", a[0], a[1], a[2]),
        pt("P2", b[0], b[1], b[2])
    )
}

/// A circle as a closed list of points, counter-clockwise (or clockwise).
fn circle(c: [f64; 2], r: f64, n: usize, cw: bool) -> Vec<[f64; 2]> {
    (0..n)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / n as f64;
            let a = if cw { -a } else { a };
            [c[0] + r * a.cos(), c[1] + r * a.sin()]
        })
        .collect()
}

/// A stadium (the oval antipad of a pair along y), clockwise, starting
/// at its left side's middle.
fn stadium(a: [f64; 2], b: [f64; 2], r: f64, n: usize) -> Vec<[f64; 2]> {
    // a above b (larger y). Clockwise from the left middle: up the left
    // side, over the top cap, down the right side, under the bottom cap.
    let mut v = Vec::new();
    let mid = (a[1] + b[1]) / 2.0;
    v.push([a[0] - r, mid]);
    for k in 0..=n {
        let t = std::f64::consts::PI * (1.0 - k as f64 / n as f64);
        v.push([a[0] + r * t.cos(), a[1] + r * t.sin()]);
    }
    for k in 0..=n {
        let t = -std::f64::consts::PI * k as f64 / n as f64;
        v.push([b[0] + r * t.cos(), b[1] + r * t.sin()]);
    }
    v
}

fn polygon(prio: u32, elev: f64, pts: &[[f64; 2]]) -> String {
    let mut o = format!(
        r#"<Polygon Priority="{prio}" Elevation="{}" NormDir="2" CoordSystem="0">"#,
        num(elev)
    );
    for p in pts {
        let _ = write!(o, r#"<Vertex X1="{}" X2="{}"/>"#, num(p[0]), num(p[1]));
    }
    o.push_str("</Polygon>");
    o
}

/// Writes the openEMS model of the transition into `dir`: `trace_via.xml`
/// (run it there with `openEMS.exe trace_via.xml`) and a short README.
/// Lengths are in mil; z is up from the top copper (Trace's own z, which
/// grows downwards, negated).
pub fn export(stackup: &Stackup, setup: &ViaSetup, dir: &Path) -> Result<Export, String> {
    let stackup = stackup_for(stackup, setup);
    let m = fdtd::model(&stackup, setup)?;
    if m.thru {
        return Err("the openEMS check needs a via (two different layers)".into());
    }
    let lay = layout(&m);
    let (x, y, z) = (&m.x, &m.y, &m.z);
    let (nx, ny, nz) = (x.len() - 1, y.len() - 1, z.len() - 1);
    let cells = nx * ny * nz;
    let zo = |k: usize| -z[k];
    let zs = |l: usize| -m.st.sheet[l];
    let (x0, x1, y0, y1) = (x[0], x[nx], y[0], y[ny]);
    let mut p = String::new();
    // Dielectric slabs.
    let fc = setup.fmax_ghz * 0.5e9;
    const EPS0: f64 = 8.854_187_812_8e-12;
    for (k, s) in m.st.slabs.iter().enumerate() {
        let kappa = if setup.lossy {
            std::f64::consts::TAU * fc * EPS0 * s.2 * s.3
        } else {
            0.0
        };
        let _ = write!(
            p,
            r#"<Material Name="slab{k}"><Property Epsilon="{}" Kappa="{}"/><Primitives>{}</Primitives></Material>"#,
            num(s.2),
            kappa,
            boxp(1, [x0, y0, -s.0], [x1, y1, -s.1])
        );
    }
    // Planes: the whole domain, less the antipads, as one polygon each
    // (the holes joined to the edge by slits of no width, clockwise).
    // Slits run at a y between grid lines, so no edge lies on one.
    let slit_y = |yc: f64| -> f64 {
        let j = y.partition_point(|&v| v <= yc).clamp(1, ny);
        y[j - 1] + 0.37 * (y[j] - y[j - 1])
    };
    let oval = setup.antipad_oval && m.sig.len() == 2;
    let n_seg = 64;
    let mut holes: Vec<(f64, Vec<[f64; 2]>)> = Vec::new();
    if oval {
        let (a, b) = if m.sig[0][1] >= m.sig[1][1] {
            (m.sig[0], m.sig[1])
        } else {
            (m.sig[1], m.sig[0])
        };
        let mut h = stadium(a, b, m.ra, n_seg / 2);
        let ys = slit_y((a[1] + b[1]) / 2.0);
        h[0] = [a[0] - m.ra, ys];
        holes.push((ys, h));
    } else {
        for c in &m.sig {
            let ys = slit_y(c[1]);
            let dy = ys - c[1];
            let dx = (m.ra * m.ra - dy * dy).max(0.0).sqrt();
            // Start at the circle's point on the slit, left side, and go
            // round clockwise.
            let a0 = std::f64::consts::PI - (dy / m.ra).clamp(-1.0, 1.0).asin();
            let mut h = vec![[c[0] - dx, ys]];
            for k in 1..n_seg {
                let a = a0 - std::f64::consts::TAU * k as f64 / n_seg as f64;
                h.push([c[0] + m.ra * a.cos(), c[1] + m.ra * a.sin()]);
            }
            holes.push((ys, h));
        }
    }
    holes.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut plane_pts: Vec<[f64; 2]> = vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
    for (ys, h) in &holes {
        plane_pts.push([x0, *ys]);
        plane_pts.extend(h.iter().copied());
        plane_pts.push(h[0]);
        plane_pts.push([x0, *ys]);
    }
    let mut metal = String::new();
    let plane_layers: Vec<usize> = (0..m.nl).filter(|&l| m.planes[l]).collect();
    for &l in &plane_layers {
        metal.push_str(&polygon(10, zs(l), &plane_pts));
    }
    // Pads on the barrel's ends and the trace layers.
    let mut pad_layers = vec![m.via_top, m.via_bot, m.from, m.to];
    pad_layers.sort();
    pad_layers.dedup();
    let rp = m.rp;
    for &l in &pad_layers {
        for c in &m.sig {
            metal.push_str(&polygon(10, zs(l), &circle(*c, rp, n_seg, false)));
        }
    }
    // Barrels.
    let cyl = |c: [f64; 2], za: f64, zb: f64, r: f64| {
        format!(
            r#"<Cylinder Priority="10" Radius="{}">{}{}</Cylinder>"#,
            num(r),
            pt("P1", c[0], c[1], za),
            pt("P2", c[0], c[1], zb)
        )
    };
    for c in &m.sig {
        metal.push_str(&cyl(*c, zs(m.via_top), zs(m.via_bot), m.rd));
    }
    for c in &m.ret {
        metal.push_str(&cyl(*c, zs(0), zs(m.nl - 1), m.rd));
    }
    // Traces: on the same grid lines the solver's cover.
    let i_via = near(x, 0.0);
    for rows in &lay.rows {
        for (t, &l) in [m.from, m.to].iter().enumerate() {
            let js = &rows[t];
            let (i0, i1) = if t == 0 {
                (lay.x_port[0], i_via)
            } else {
                (i_via, lay.x_port[1])
            };
            metal.push_str(&boxp(
                10,
                [x[i0], y[js[0]], zs(l)],
                [x[i1], y[*js.last().unwrap()], zs(l)],
            ));
        }
    }
    let _ = write!(
        p,
        r#"<Metal Name="copper"><Primitives>{metal}</Primitives></Metal>"#
    );
    // Ports: a resistor from each trace's end to each of its reference
    // planes (z_ref times the planes' count, so they make z_ref
    // together); the first trace's driven (a pair's second leg opposite).
    let z_ref = 50.0;
    let dual = |v: &[f64], i: usize| (v[i] + v[i + 1]) / 2.0;
    for (si, rows) in lay.rows.iter().enumerate() {
        let polarity = if si == 0 { 1.0 } else { -1.0 };
        for t in 0..2 {
            let l = [m.from, m.to][t];
            let r = [m.r_from, m.r_to][t];
            let refs: Vec<usize> = [r.0, r.1].into_iter().flatten().collect();
            let i = lay.x_port[t];
            let jc = rows[t][rows[t].len() / 2];
            for (q, &pl) in refs.iter().enumerate() {
                let (zt, zp) = (zs(l), zs(pl));
                let a = [x[i], y[jc], zp];
                let b = [x[i], y[jc], zt];
                let _ = write!(
                    p,
                    r#"<LumpedElement Name="R_{si}_{t}_{q}" Direction="2" Caps="0" R="{}"><Primitives>{}</Primitives></LumpedElement>"#,
                    num(z_ref * refs.len() as f64),
                    boxp(5, a, b)
                );
                if t == 0 {
                    // E from the trace to the plane raises the trace.
                    let dir = if zp < zt { -1.0 } else { 1.0 } * polarity;
                    let _ = write!(
                        p,
                        r#"<Excitation Name="src_{si}_{q}" Type="0" Excite="0,0,{}"><Primitives>{}</Primitives></Excitation>"#,
                        num(dir),
                        boxp(5, a, b)
                    );
                }
            }
        }
    }
    // Probes: V of the trace over its plane (below, else above) on the
    // two grid lines either side of the solver's current loop, and the
    // current through a loop one cell round the trace (on the dual grid).
    for (si, rows) in lay.rows.iter().enumerate() {
        for pr in 0..4 {
            let t = pr % 2;
            let l = [m.from, m.to][t];
            let r = [m.r_from, m.r_to][t];
            let pl = r.0.or(r.1).unwrap();
            let i = lay.x_probe[pr];
            let js = &rows[t];
            let jc = js[js.len() / 2];
            for (q, ii) in [i, i + 1].into_iter().enumerate() {
                // Like a lumped port's: from the plane up to the trace,
                // weight −1, so V is the trace's potential over the plane.
                let _ = write!(
                    p,
                    r#"<ProbeBox Name="v_{si}_{pr}_{q}" Type="0" Weight="-1"><Primitives>{}</Primitives></ProbeBox>"#,
                    boxp(0, [x[ii], y[jc], zs(pl)], [x[ii], y[jc], zs(l)])
                );
            }
            let k = lay.ksheet[l];
            let (jl, jr) = (js[0] - 1, *js.last().unwrap());
            let xi = dual(x, i);
            let _ = write!(
                p,
                r#"<ProbeBox Name="i_{si}_{pr}" Type="1" Weight="1" NormDir="0"><Primitives>{}</Primitives></ProbeBox>"#,
                boxp(
                    0,
                    [xi, dual(y, jl), -dual(z, k)],
                    [xi, dual(y, jr), -dual(z, k - 1)]
                )
            );
        }
    }
    let fmax = setup.fmax_ghz * 1e9;
    let mut o = String::new();
    let _ = writeln!(
        o,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?>"#
    );
    let _ = writeln!(
        o,
        "<!-- Via transition exported by Trace {} for openEMS. Lengths in mil; z up from the top copper. -->",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(o, "<openEMS>");
    let _ = writeln!(
        o,
        r#"<FDTD NumberOfTimesteps="2000000" endCriteria="1e-05" f_max="{fmax:e}">"#
    );
    let _ = writeln!(
        o,
        r#"<Excitation Type="0" f0="{:e}" fc="{:e}"/>"#,
        fmax / 2.0,
        fmax / 2.0
    );
    let _ = writeln!(
        o,
        r#"<BoundaryCond xmin="PML_8" xmax="PML_8" ymin="PML_8" ymax="PML_8" zmin="PML_8" zmax="PML_8"/>"#
    );
    let _ = writeln!(o, "</FDTD>");
    let _ = writeln!(o, r#"<ContinuousStructure CoordSystem="0">"#);
    let _ = writeln!(o, "<Properties>\n{p}\n</Properties>");
    // z lines from the top of the domain down, negated (openEMS wants
    // them rising: it sorts them anyway).
    let mut zr: Vec<f64> = z.iter().map(|v| -v).collect();
    zr.reverse();
    let _ = writeln!(
        o,
        r#"<RectilinearGrid DeltaUnit="{:e}" CoordSystem="0"><XLines>{}</XLines><YLines>{}</YLines><ZLines>{}</ZLines></RectilinearGrid>"#,
        MIL,
        lines(x, false),
        lines(y, false),
        lines(&zr, false)
    );
    let _ = writeln!(o, "</ContinuousStructure>\n</openEMS>");
    let _ = zo;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let xml = dir.join(MODEL);
    std::fs::write(&xml, o).map_err(|e| format!("{}: {e}", xml.display()))?;
    let readme = format!(
        "Via transition exported by Trace {} for openEMS.\r\n\r\n\
         Run here:   openEMS.exe {MODEL}\r\n\
         Then read the results back with Trace:\r\n\
         \x20 trace via <board> <from> <to> <same options> --openems-read <this folder>\r\n\r\n\
         Grid {nx} x {ny} x {nz} = {cells} cells, smallest cell {:.2} mil, PML 8 cells each side.\r\n\
         Gaussian pulse to {} GHz; stops when the energy is 50 dB down.\r\n\
         Probes v_S_P_Q (voltage) and i_S_P (current): signal via S, probe P\r\n\
         (0, 1 next to the via on the incoming and outgoing trace, 2, 3 further out).\r\n",
        env!("CARGO_PKG_VERSION"),
        m.d0,
        setup.fmax_ghz
    );
    let _ = std::fs::write(dir.join("README.txt"), readme);
    let mut notes = vec![format!(
        "openEMS grid {nx}x{ny}x{nz} ({:.1} M cells), smallest cell {:.2} mil, round metal in steps",
        cells as f64 / 1e6,
        m.d0
    )];
    if cells > 40_000_000 {
        notes.push("a very large grid: openEMS may need a lot of memory".into());
    }
    Ok(Export {
        dir: dir.to_path_buf(),
        xml,
        cells,
        cell: m.d0,
        notes,
    })
}

/// Where openEMS is: `hint` (the program or its folder), else
/// `TRACE_OPENEMS`, else the usual install folders, else the PATH.
pub fn find_exe(hint: Option<&Path>) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        "openEMS.exe"
    } else {
        "openEMS"
    };
    let try_at = |p: &Path| -> Option<PathBuf> {
        if p.is_file() {
            return Some(p.to_path_buf());
        }
        [
            p.join(exe),
            p.join("openEMS").join(exe),
            p.join("bin").join(exe),
        ]
        .into_iter()
        .find(|q| q.is_file())
    };
    if let Some(h) = hint {
        return try_at(h);
    }
    if let Some(e) = std::env::var_os("TRACE_OPENEMS") {
        if let Some(p) = try_at(Path::new(&e)) {
            return Some(p);
        }
    }
    let mut spots: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for d in [
            "C:\\openEMS",
            "C:\\Program Files\\openEMS",
            "C:\\opt\\openEMS",
        ] {
            spots.push(PathBuf::from(d));
        }
        if let Some(h) = std::env::var_os("USERPROFILE") {
            spots.push(Path::new(&h).join("openEMS"));
        }
    } else {
        for d in ["/usr/local/bin", "/usr/bin", "/opt/openEMS/bin"] {
            spots.push(PathBuf::from(d));
        }
        if let Some(h) = std::env::var_os("HOME") {
            spots.push(Path::new(&h).join("opt/openEMS/bin"));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        spots.extend(std::env::split_paths(&path));
    }
    spots.iter().find_map(|d| try_at(d))
}

/// The energy openEMS last reported in its log, dB (0 at the start,
/// falling to −50 when it stops).
fn last_energy_db(log: &Path) -> Option<f64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(log).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(2000))).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    // "... || Energy: ~1.23e-10 (- 12.34dB)"
    let at = s.rfind("(-")?;
    let tail = &s[at + 2..];
    let end = tail.find("dB")?;
    tail[..end].trim().parse::<f64>().ok().map(|v| -v)
}

/// Runs openEMS on the model in `dir` (its probes land there, its log in
/// `openems.log`), reporting progress (0 to 1) from the energy left and
/// stopping it when the job is cancelled.
pub fn run(exe: &Path, dir: &Path, progress: &dyn Fn(f64)) -> Result<(), String> {
    let log = dir.join("openems.log");
    let out = std::fs::File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(MODEL)
        .current_dir(dir)
        .stdout(out)
        .stderr(err)
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", exe.display()))?;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                if st.success() {
                    progress(1.0);
                    return Ok(());
                }
                let tail = std::fs::read_to_string(&log).unwrap_or_default();
                let tail: Vec<&str> = tail.lines().rev().take(8).collect();
                return Err(format!(
                    "openEMS stopped with {st}: {}",
                    tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
                ));
            }
            Ok(None) => {}
            Err(e) => return Err(e.to_string()),
        }
        if let Err(e) = crate::cancel::check() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        if let Some(db) = last_energy_db(&log) {
            progress((-db / 50.0).clamp(0.0, 0.99));
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// One probe file: (time s, value) rows after `%` comment lines.
fn probe(dir: &Path, name: &str) -> Result<Vec<(f64, f64)>, String> {
    let path = dir.join(name);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("{}: {e} (has openEMS run there?)", path.display()))?;
    let mut v = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('%') || l.starts_with('#') {
            continue;
        }
        let mut it = l.split_whitespace().map(|t| t.parse::<f64>());
        if let (Some(Ok(t)), Some(Ok(u))) = (it.next(), it.next()) {
            v.push((t, u));
        }
    }
    if v.len() < 10 {
        return Err(format!("{}: too few samples", path.display()));
    }
    Ok(v)
}

/// Fourier transform of samples at their own times.
fn dft(s: &[(f64, f64)], freqs: &[f64]) -> Vec<Cx> {
    freqs
        .iter()
        .map(|&f| {
            let w = std::f64::consts::TAU * f;
            let (mut re, mut im) = (0.0, 0.0);
            for &(t, u) in s {
                let (sn, cs) = (w * t).sin_cos();
                re += u * cs;
                im -= u * sn;
            }
            Cx::new(re, im)
        })
        .collect()
}

/// Reads openEMS's probes from `dir` (after a run of the model `export`
/// wrote for this `setup`) into a via result.
pub fn read(stackup: &Stackup, setup: &ViaSetup, dir: &Path) -> Result<ViaResult, String> {
    let stackup = stackup_for(stackup, setup);
    let m = fdtd::model(&stackup, setup)?;
    let lay = layout(&m);
    let ns = m.sig.len();
    let nf = setup.points.max(10);
    let freqs: Vec<f64> = (1..=nf)
        .map(|k| setup.fmax_ghz * 1e9 * k as f64 / nf as f64)
        .collect();
    let mut fv = vec![Vec::new(); 4 * ns];
    let mut fi = vec![Vec::new(); 4 * ns];
    let mut samples = 0;
    for s in 0..ns {
        for pr in 0..4 {
            let a = probe(dir, &format!("v_{s}_{pr}_0"))?;
            let b = probe(dir, &format!("v_{s}_{pr}_1"))?;
            let (fa, fb) = (dft(&a, &freqs), dft(&b, &freqs));
            fv[pr * ns + s] = fa
                .iter()
                .zip(&fb)
                .map(|(p, q)| p.add(*q).scale(0.5))
                .collect();
            let c = probe(dir, &format!("i_{s}_{pr}"))?;
            samples = samples.max(c.len());
            fi[pr * ns + s] = dft(&c, &freqs);
        }
    }
    // Each trace carries waves away from the source, so V/I is about +Z
    // on it: where openEMS's sign conventions say otherwise (a probe's
    // start and stop taken the other way round), turn its V over.
    let mut flipped = 0;
    for s in 0..ns {
        for t in 0..2 {
            let probes = [t, t + 2];
            let mut sum = 0.0;
            for &pr in &probes {
                let (v, i) = (&fv[pr * ns + s], &fi[pr * ns + s]);
                for f in nf / 8..nf / 2 {
                    sum += v[f].re * i[f].re + v[f].im * i[f].im;
                }
            }
            if sum < 0.0 {
                flipped += 1;
                for &pr in &probes {
                    for c in fv[pr * ns + s].iter_mut() {
                        *c = c.scale(-1.0);
                    }
                }
            }
        }
    }
    let xh = |i: usize| (m.x[i] + m.x[i + 1]) / 2.0 * MIL;
    let xp = [
        xh(lay.x_probe[0]),
        xh(lay.x_probe[1]),
        xh(lay.x_probe[2]),
        xh(lay.x_probe[3]),
    ];
    let mut notes = vec![format!(
        "openEMS result read from {} ({samples} samples a probe{})",
        dir.display(),
        if flipped > 0 {
            format!("; {flipped} trace's voltage taken with the other sign")
        } else {
            String::new()
        }
    )];
    let e = fdtd::extract(
        &m.st, setup, m.from, m.to, &freqs, &fv, &fi, ns, m.z0, m.eeff, xp, false, &mut notes,
    );
    if e.power > 1.005 {
        notes.push(format!(
            "openEMS gave up to {:.1} % more power out than in (left as it is)",
            (e.power - 1.0) * 100.0
        ));
    }
    let (nx, ny, nz) = (m.x.len() - 1, m.y.len() - 1, m.z.len() - 1);
    Ok(ViaResult {
        fields: Vec::new(),
        f: freqs.iter().map(|f| f / 1e9).collect(),
        s11: e.s11,
        s21: e.s21,
        z_ref: if ns == 2 { 100.0 } else { 50.0 },
        w: m.w,
        z0: m.z0,
        z_line: e.zline,
        stub: m.stub,
        cells: nx * ny * nz,
        steps: samples,
        cell: m.d0,
        power: e.power,
        z_via: e.z_via,
        notes,
        err11: Vec::new(),
        err21: Vec::new(),
    })
}

/// Exports, runs and reads back: the openEMS result for `setup` (on
/// `cell` mil, or half Trace's smallest cell when 0).
pub fn check(
    stackup: &Stackup,
    setup: &ViaSetup,
    cell: f64,
    dir: &Path,
    exe: &Path,
    progress: &dyn Fn(f64),
) -> Result<(ViaResult, Export), String> {
    let s = setup_for(stackup, setup, cell)?;
    let ex = export(stackup, &s, dir)?;
    run(exe, dir, progress)?;
    let mut r = read(stackup, &s, dir)?;
    let mut notes = ex.notes.clone();
    notes.append(&mut r.notes);
    r.notes = notes;
    Ok((r, ex))
}

/// How far the two solvers' results are apart.
#[derive(Clone, Debug)]
pub struct Compare {
    /// Largest difference in |S11| and |S21|, dB, over the band (S11
    /// only where either is above −40 dB, as deeper return loss is noise).
    pub d11_db: f64,
    pub d21_db: f64,
    /// Where they fall.
    pub f11: f64,
    pub f21: f64,
    pub z_trace: Option<f64>,
    pub z_openems: Option<f64>,
}

/// Index of the point nearest `f` GHz.
fn at(r: &ViaResult, f: f64) -> usize {
    (0..r.f.len())
        .min_by(|&a, &b| (r.f[a] - f).abs().total_cmp(&(r.f[b] - f).abs()))
        .unwrap_or(0)
}

pub fn compare(t: &ViaResult, o: &ViaResult) -> Compare {
    let (mut d11, mut f11, mut d21, mut f21) = (0.0f64, 0.0, 0.0f64, 0.0);
    for (k, &f) in t.f.iter().enumerate() {
        let j = at(o, f);
        let (a, b) = (t.s11[k].db(), o.s11[j].db());
        if a.max(b) > -40.0 && (a - b).abs() > d11 {
            d11 = (a - b).abs();
            f11 = f;
        }
        let (a, b) = (t.s21[k].db(), o.s21[j].db());
        if (a - b).abs() > d21 {
            d21 = (a - b).abs();
            f21 = f;
        }
    }
    Compare {
        d11_db: d11,
        d21_db: d21,
        f11,
        f21,
        z_trace: t.z_via,
        z_openems: o.z_via,
    }
}

impl Compare {
    pub fn summary(&self) -> String {
        let z = match (self.z_trace, self.z_openems) {
            (Some(a), Some(b)) => format!(
                "; via impedance {a:.1} Ω (Trace) vs {b:.1} Ω (openEMS), {:+.1} %",
                (b - a) / a * 100.0
            ),
            _ => String::new(),
        };
        format!(
            "|S11| apart by up to {:.2} dB (at {:.2} GHz), |S21| by up to {:.3} dB (at {:.2} GHz){z}",
            self.d11_db, self.f11, self.d21_db, self.f21
        )
    }
}

/// A table of both results at about `rows` frequencies (and at `extra`).
pub fn table(t: &ViaResult, o: &ViaResult, rows: usize, extra: &[f64]) -> String {
    let kind = if t.z_ref > 60.0 { "Sdd" } else { "S" };
    let mut s = format!(
        "  {:>7}  {:>10} {:>10} {:>7}   {:>10} {:>10} {:>7}\n",
        "GHz",
        format!("{kind}11 Trace"),
        "openEMS",
        "diff",
        format!("{kind}21 Trace"),
        "openEMS",
        "diff"
    );
    let n = t.f.len();
    let mut ks: Vec<usize> = (0..n)
        .filter(|k| (k + 1) % (n / rows).max(1) == 0)
        .collect();
    for &f in extra {
        ks.push(at(t, f));
    }
    ks.sort();
    ks.dedup();
    for k in ks {
        let j = at(o, t.f[k]);
        let (a, b, c, d) = (t.s11[k].db(), o.s11[j].db(), t.s21[k].db(), o.s21[j].db());
        let _ = writeln!(
            s,
            "  {:>7.2}  {a:>10.2} {b:>10.2} {:>+7.2}   {c:>10.3} {d:>10.3} {:>+7.3}",
            t.f[k],
            b - a,
            d - c
        );
    }
    s
}

/// Both results' |S11| and |S21| in dB as SVG: Trace solid, openEMS
/// dashed, with how far apart they are.
pub fn svg(t: &ViaResult, o: &ViaResult, title: &str) -> String {
    let (w, h, l, r, top, b) = (900.0, 540.0, 60.0, 20.0, 40.0, 70.0);
    let fmax = t.f.last().copied().unwrap_or(1.0);
    let min_db = t
        .s11
        .iter()
        .chain(&o.s11)
        .chain(&t.s21)
        .chain(&o.s21)
        .map(|s| s.db())
        .fold(0.0f64, f64::min)
        .max(-60.0);
    let lo = (min_db / 10.0).floor() * 10.0;
    let x = |f: f64| l + (w - l - r) * f / fmax;
    let y = |d: f64| top + (h - top - b) * (d.min(0.0) / lo);
    let mut s = String::new();
    let _ = writeln!(
        s,
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" font-family="Helvetica, Arial, sans-serif" font-size="12"><rect width="100%" height="100%" fill="#ffffff"/>"##
    );
    let mut d = 0.0;
    while d >= lo - 1e-9 {
        let _ = writeln!(
            s,
            r##"<line x1="{l}" x2="{}" y1="{:.1}" y2="{:.1}" stroke="#e3e6ec"/><text x="{}" y="{:.1}" text-anchor="end" fill="#555">{d:.0}</text>"##,
            w - r,
            y(d),
            y(d),
            l - 6.0,
            y(d) + 4.0
        );
        d -= 10.0;
    }
    let step = [1.0, 2.0, 5.0, 10.0, 20.0]
        .into_iter()
        .find(|st| fmax / st <= 10.0)
        .unwrap_or(50.0);
    let mut f = 0.0;
    while f <= fmax + 1e-9 {
        let _ = writeln!(
            s,
            r##"<line x1="{:.1}" x2="{:.1}" y1="{top}" y2="{}" stroke="#e3e6ec"/><text x="{:.1}" y="{}" text-anchor="middle" fill="#555">{f:.0}</text>"##,
            x(f),
            x(f),
            h - b,
            x(f),
            h - b + 16.0
        );
        f += step;
    }
    let curves: [(&ViaResult, &Vec<Cx>, &str, &str, &str); 4] = [
        (t, &t.s21, "#1f6feb", "", "|S21| Trace"),
        (
            o,
            &o.s21,
            "#1f6feb",
            r#" stroke-dasharray="6 4""#,
            "|S21| openEMS",
        ),
        (t, &t.s11, "#d9480f", "", "|S11| Trace"),
        (
            o,
            &o.s11,
            "#d9480f",
            r#" stroke-dasharray="6 4""#,
            "|S11| openEMS",
        ),
    ];
    for (k, (res, v, color, dash, name)) in curves.iter().enumerate() {
        let pts: Vec<String> = res
            .f
            .iter()
            .zip(v.iter())
            .map(|(f, c)| format!("{:.1},{:.1}", x(*f), y(c.db().max(lo))))
            .collect();
        let _ = writeln!(
            s,
            r##"<polyline points="{}" fill="none" stroke="{color}" stroke-width="1.8"{dash}/>"##,
            pts.join(" ")
        );
        let ly = top + 14.0 + 16.0 * k as f64;
        let _ = writeln!(
            s,
            r##"<line x1="{}" x2="{}" y1="{ly}" y2="{ly}" stroke="{color}" stroke-width="2"{dash}/><text x="{}" y="{}" fill="#222">{name}</text>"##,
            w - r - 170.0,
            w - r - 140.0,
            w - r - 134.0,
            ly + 4.0
        );
    }
    let c = compare(t, o);
    let _ = writeln!(
        s,
        r##"<text x="{l}" y="24" font-size="14" fill="#111">{}</text><text x="{}" y="{}" text-anchor="middle" fill="#555">GHz</text><text x="{l}" y="{}" fill="#333">{}</text><text x="16" y="{}" fill="#555" transform="rotate(-90 16 {})">dB</text></svg>"##,
        title.replace('&', "&amp;").replace('<', "&lt;"),
        (l + w - r) / 2.0,
        h - b + 32.0,
        h - 12.0,
        c.summary().replace('&', "&amp;").replace('<', "&lt;"),
        (top + h - b) / 2.0,
        (top + h - b) / 2.0
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{units::Length, LayerKind, StackLayer};

    fn stack() -> Stackup {
        let layer = |n: &str, kind: LayerKind, t: f64, er: Option<f64>| StackLayer {
            name: n.into(),
            altium_id: None,
            kind,
            thickness: Length::from_mil(t),
            dielectric_constant: er,
            loss_tangent: None,
            roughness_um: None,
            material: None,
        };
        let cu = |n: &str| layer(n, LayerKind::Copper, 1.4, None);
        let di = |t: f64| layer("core", LayerKind::Dielectric, t, Some(3.7));
        Stackup {
            layers: vec![
                cu("TOP"),
                di(5.0),
                cu("GND1"),
                di(10.0),
                cu("SIG3"),
                di(10.0),
                cu("GND4"),
                di(5.0),
                cu("BOT"),
            ],
        }
    }

    #[test]
    fn exports_a_pair() {
        let st = stack();
        let mut v = ViaSetup::new(0, 2);
        v.pair = Some(40.0);
        v.antipad_oval = true;
        v.fmax_ghz = 20.0;
        let dir = std::env::temp_dir().join(format!("trace_oems_{}", std::process::id()));
        let s = setup_for(&st, &v, 0.0).unwrap();
        let ex = export(&st, &s, &dir).unwrap();
        let xml = std::fs::read_to_string(&ex.xml).unwrap();
        assert!(xml.contains("<openEMS>") && xml.contains("</openEMS>"));
        // Two planes, pads on three layers for each via, two driven
        // resistors, 16 V and 8 I probes.
        assert_eq!(xml.matches("<Polygon").count(), 2 + 3 * 2);
        assert_eq!(xml.matches("<Excitation Name").count(), 2);
        assert_eq!(xml.matches(r#"Type="0" Weight"#).count(), 16);
        assert_eq!(xml.matches(r#"Type="1" Weight"#).count(), 8);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_probes_back() {
        // Synthetic probes of a matched line: forward waves only, so the
        // extraction should give S21 near 1 and S11 near 0.
        let st = stack();
        let mut v = ViaSetup::new(0, 0);
        v.fmax_ghz = 20.0;
        v.points = 40;
        v.metal_loss = false;
        let dir = std::env::temp_dir().join(format!("trace_oems_rd_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let m = fdtd::model(&st, &v).unwrap();
        let lay = layout(&m);
        let z = 50.0;
        let vel = 299_792_458.0 / m.eeff[0].sqrt();
        let pulse = |t: f64| {
            let tau = 2e-11;
            (-((t - 2e-10) / tau).powi(2)).exp()
        };
        let xh = |i: usize| (m.x[i] + m.x[i + 1]) / 2.0 * MIL;
        for pr in 0..4 {
            let xp = xh(lay.x_probe[pr]);
            let mut vs = String::from("% t u\n");
            let mut is = String::from("% t i\n");
            for n in 0..4000 {
                let t = n as f64 * 1e-13;
                let u = pulse(t - xp / vel);
                let _ = writeln!(vs, "{t:e}\t{u:e}");
                let _ = writeln!(is, "{t:e}\t{:e}", u / z);
            }
            for q in 0..2 {
                std::fs::write(dir.join(format!("v_0_{pr}_{q}")), &vs).unwrap();
            }
            std::fs::write(dir.join(format!("i_0_{pr}")), &is).unwrap();
        }
        let r = read(&st, &v, &dir).unwrap();
        let k = r.f.len() / 2;
        assert!(r.s11[k].abs() < 0.05, "S11 {}", r.s11[k].abs());
        assert!(
            (r.s21[k].abs() - 1.0).abs() < 0.05,
            "S21 {}",
            r.s21[k].abs()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
