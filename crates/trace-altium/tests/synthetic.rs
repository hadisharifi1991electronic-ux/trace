//! Builds tiny Altium files in memory so tests never need real designs.

use std::io::Write;

fn text_record(text: &str) -> Vec<u8> {
    let mut v = (text.len() as u32 + 1).to_le_bytes().to_vec();
    v.extend_from_slice(text.as_bytes());
    v.push(0);
    v
}

/// A copper track record: layer, flags, net, links, end points, width.
fn track(layer: u8, net: u16, (x1, y1): (f64, f64), (x2, y2): (f64, f64)) -> Vec<u8> {
    let mil = |v: f64| ((v * 10_000.0) as i32).to_le_bytes();
    let mut body = vec![layer, 12, 0];
    body.extend_from_slice(&net.to_le_bytes());
    body.extend_from_slice(&[0xFF; 8]);
    for c in [mil(x1), mil(y1), mil(x2), mil(y2), mil(5.0)] {
        body.extend_from_slice(&c);
    }
    let mut rec = vec![4];
    rec.extend_from_slice(&(body.len() as u32).to_le_bytes());
    rec.extend(body);
    rec
}

fn write_cfb(path: &std::path::Path, streams: &[(&str, Vec<u8>)]) {
    let mut c = cfb::create(path).unwrap();
    for (name, data) in streams {
        if let Some(parent) = std::path::Path::new(name).parent() {
            if parent != std::path::Path::new("/") {
                c.create_storage_all(parent).unwrap();
            }
        }
        c.create_stream(name).unwrap().write_all(data).unwrap();
    }
    c.flush().unwrap();
}

#[test]
fn reads_board_stackup_nets_and_pair_lengths() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("demo.PcbDoc");
    let board = "|V9_STACK_LAYER0_NAME=Top|V9_STACK_LAYER0_LAYERID=16777217|V9_STACK_LAYER0_COPTHICK=1.4mil\
|V9_STACK_LAYER1_NAME=Core|V9_STACK_LAYER1_LAYERID=17039361|V9_STACK_LAYER1_DIELHEIGHT=8mil|V9_STACK_LAYER1_DIELCONST=4.2\
|V9_STACK_LAYER2_NAME=Bottom|V9_STACK_LAYER2_LAYERID=16777248|V9_STACK_LAYER2_COPTHICK=1.4mil";
    let mut nets = text_record("|NAME=CLK_P");
    nets.extend(text_record("|NAME=CLK_N"));
    let mut tracks = track(1, 0, (0.0, 0.0), (300.0, 400.0)); // 500 mil
    tracks.extend(track(1, 1, (0.0, 0.0), (490.0, 0.0)));
    tracks.extend(track(33, 1, (0.0, 0.0), (1000.0, 0.0))); // silkscreen, ignored
    write_cfb(
        &path,
        &[
            ("/Board6/Data", text_record(board)),
            ("/Nets6/Data", nets),
            (
                "/DifferentialPairs6/Data",
                text_record("|NAME=CLK|POSITIVENETNAME=CLK_P|NEGATIVENETNAME=CLK_N"),
            ),
            ("/Tracks6/Data", tracks),
        ],
    );

    let b = trace_altium::read_pcbdoc(&path).unwrap();
    assert_eq!(b.stackup.copper_layers().count(), 2);
    assert_eq!(b.stackup.layers[1].dielectric_constant, Some(4.2));
    assert!((b.stackup.layers[1].thickness.mil() - 8.0).abs() < 1e-9);
    assert_eq!(b.stats.tracks, 3);
    assert!((b.net("CLK_P").unwrap().routed_length.mil() - 500.0).abs() < 0.01);
    assert!((b.net("CLK_N").unwrap().routed_length.mil() - 490.0).abs() < 0.01);
    assert_eq!(b.diff_pairs[0].positive, "CLK_P");
}

#[test]
fn reads_schematic_symbols() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("demo.SchDoc");
    let mut data =
        text_record("|HEADER=Protel for Windows - Schematic Capture Binary File Version 5.0");
    data.extend(text_record(
        "|RECORD=1|LibReference=RES|ComponentDescription=Resistor",
    ));
    data.extend(text_record("|RECORD=2|OwnerIndex=0|Name=1"));
    data.extend(text_record("|RECORD=2|OwnerIndex=0|Name=2"));
    data.extend(text_record(
        "|RECORD=34|OwnerIndex=0|Name=Designator|Text=R1",
    ));
    data.extend(text_record("|RECORD=41|OwnerIndex=0|Name=Value|Text=10k"));
    data.extend(text_record("|RECORD=25|Text=SDA"));
    data.extend(text_record("|RECORD=27"));
    write_cfb(&path, &[("/FileHeader", data)]);

    let s = trace_altium::read_schdoc(&path).unwrap();
    assert_eq!(s.symbols.len(), 1);
    let r = &s.symbols[0];
    assert_eq!(
        (r.designator.as_str(), r.value.as_deref(), r.pins),
        ("R1", Some("10k"), 2)
    );
    assert_eq!(s.net_labels, vec!["SDA"]);
    assert_eq!(s.wires, 1);
}

/// A small sheet: a resistor with two pins and a body, wires to a net
/// label, a power port and a port, a junction, a note and a hidden
/// footprint model.
fn demo_sheet(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("demo.SchDoc");
    let mut data = text_record(
        "|HEADER=Protel for Windows - Schematic Capture Binary File Version 5.0|WEIGHT=16",
    );
    for r in [
        "|RECORD=31|FONTIDCOUNT=1|SIZE1=10|FONTNAME1=Times New Roman|SHEETSTYLE=0|BORDERON=T|AREACOLOR=16317695|SYSTEMFONT=1",
        "|RECORD=1|LIBREFERENCE=RES|COMPONENTDESCRIPTION=Resistor|LOCATION.X=100|LOCATION.Y=100|CURRENTPARTID=1|PARTCOUNT=2|UNIQUEID=ABCDEFGH",
        "|RECORD=14|OWNERINDEX=1|OWNERPARTID=1|LOCATION.X=100|LOCATION.Y=96|CORNER.X=130|CORNER.Y=104|COLOR=128|AREACOLOR=11599871|ISSOLID=T|LINEWIDTH=1",
        "|RECORD=2|OWNERINDEX=1|OWNERPARTID=1|LOCATION.X=100|LOCATION.Y=100|PINLENGTH=10|PINCONGLOMERATE=34|NAME=1|DESIGNATOR=1|ELECTRICAL=4",
        "|RECORD=2|OWNERINDEX=1|OWNERPARTID=1|LOCATION.X=130|LOCATION.Y=100|PINLENGTH=10|PINCONGLOMERATE=32|NAME=2|DESIGNATOR=2|ELECTRICAL=4",
        "|RECORD=34|OWNERINDEX=1|OWNERPARTID=-1|LOCATION.X=110|LOCATION.Y=110|NAME=Designator|TEXT=R1|FONTID=1|COLOR=8388608",
        "|RECORD=41|OWNERINDEX=1|OWNERPARTID=-1|LOCATION.X=110|LOCATION.Y=86|NAME=Value|TEXT=10k|FONTID=1|COLOR=8388608",
        "|RECORD=41|OWNERINDEX=1|OWNERPARTID=-1|LOCATION.X=110|LOCATION.Y=86|NAME=Tolerance|TEXT=1%|ISHIDDEN=T",
        "|RECORD=44|OWNERINDEX=1",
        "|RECORD=45|OWNERINDEX=8|MODELNAME=R0402|MODELTYPE=PCBLIB|ISCURRENT=T|DATAFILECOUNT=1|MODELDATAFILEENTITY0=R0402|MODELDATAFILEKIND0=PCBLib",
        "|RECORD=27|LOCATIONCOUNT=2|X1=90|Y1=100|X2=60|Y2=100|COLOR=8388608|LINEWIDTH=1",
        "|RECORD=27|LOCATIONCOUNT=3|X1=140|Y1=100|X2=170|Y2=100|X3=170|Y3=70|COLOR=8388608|LINEWIDTH=1",
        "|RECORD=27|LOCATIONCOUNT=2|X1=170|Y1=100|X2=200|Y2=100|COLOR=8388608|LINEWIDTH=1",
        "|RECORD=29|LOCATION.X=170|LOCATION.Y=100|COLOR=8388608",
        "|RECORD=25|LOCATION.X=60|LOCATION.Y=100|TEXT=SDA|FONTID=1|COLOR=128",
        "|RECORD=17|LOCATION.X=170|LOCATION.Y=70|ORIENTATION=3|STYLE=4|TEXT=GND|SHOWNETNAME=T|COLOR=128",
        "|RECORD=18|LOCATION.X=200|LOCATION.Y=100|WIDTH=50|HEIGHT=10|NAME=OUT|STYLE=3|IOTYPE=2|ALIGNMENT=2|COLOR=128|AREACOLOR=8454143|TEXTCOLOR=128",
        "|RECORD=28|LOCATION.X=30|LOCATION.Y=30|CORNER.X=90|CORNER.Y=50|TEXT=A note~1second line|ISSOLID=T|SHOWBORDER=T|AREACOLOR=16777215|COLOR=0|FONTID=1|WORDWRAP=T|TEXTMARGIN=5|ALIGNMENT=1",
    ] {
        data.extend(text_record(r));
    }
    let mut extra =
        text_record("|HEADER=Protel for Windows - Schematic Capture Binary File Version 5.0");
    extra.extend(text_record(
        "|RECORD=225|LOCATIONCOUNT=4|X1=20|Y1=20|X2=220|Y2=20|X3=220|Y3=120|X4=20|Y4=120|COLOR=16711680|LINEWIDTH=1|UNIQUEID=BLANKETA",
    ));
    write_cfb(&path, &[("/FileHeader", data), ("/Additional", extra)]);
    path
}

fn json(s: &trace_core::Sheet) -> serde_json::Value {
    let mut v = serde_json::to_value(s).unwrap();
    v["name"] = serde_json::Value::Null;
    v
}

/// What a sheet draws, in no particular order.
fn drawn(s: &trace_core::Sheet) -> Vec<Vec<String>> {
    let d = &s.drawing;
    let each = |v: serde_json::Value| -> Vec<String> {
        let mut out: Vec<String> = v
            .as_array()
            .unwrap()
            .iter()
            .map(|x| {
                let mut x = x.clone();
                // Index fields point into lists whose order may differ.
                for k in ["wire", "graphic", "symbol"] {
                    if let Some(o) = x.as_object_mut() {
                        o.remove(k);
                    }
                }
                x.to_string()
            })
            .collect();
        out.sort();
        out
    };
    vec![
        each(serde_json::to_value(&d.shapes).unwrap()),
        each(serde_json::to_value(&d.texts).unwrap()),
        each(serde_json::to_value(&d.pins).unwrap()),
        each(serde_json::to_value(&d.wires).unwrap()),
        each(serde_json::to_value(&d.tags).unwrap()),
        each(serde_json::to_value(&d.parts).unwrap()),
        each(serde_json::to_value(&s.symbols).unwrap()),
    ]
}

#[test]
fn schematic_sheets_write_back_as_they_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = demo_sheet(dir.path());
    let sheet = trace_altium::read_schdoc(&path).unwrap();
    assert_eq!(sheet.symbols.len(), 1);
    assert!(sheet.drawing.shapes.len() > 5);

    // With the original file and without it, the sheet reads back the same.
    for original in [Some(path.as_path()), None] {
        let out = dir.path().join("out.SchDoc");
        std::fs::write(&out, trace_altium::write_schdoc(&sheet, original).unwrap()).unwrap();
        let back = trace_altium::read_schdoc(&out).unwrap();
        if original.is_some() {
            // Kept in the file's own order, so exactly as read.
            assert_eq!(json(&back), json(&sheet));
        }
        assert_eq!(drawn(&back), drawn(&sheet), "original {original:?}");
        assert_eq!(back.symbols[0].footprint, "R0402");
    }

    // Records the drawing does not hold survive: the hidden tolerance.
    let out = dir.path().join("kept.SchDoc");
    std::fs::write(
        &out,
        trace_altium::write_schdoc(&sheet, Some(&path)).unwrap(),
    )
    .unwrap();
    let text = String::from_utf8_lossy(&std::fs::read(&out).unwrap()).into_owned();
    assert!(text.contains("TEXT=1%") && text.contains("NAME=Tolerance"));
    assert!(text.contains("UNIQUEID=ABCDEFGH"));
    assert!(text.contains("UNIQUEID=BLANKETA"));
}

#[test]
fn edited_schematic_sheets_write_their_edits() {
    let dir = tempfile::tempdir().unwrap();
    let path = demo_sheet(dir.path());
    let mut sheet = trace_altium::read_schdoc(&path).unwrap();
    sheet.drawing.move_part(0, [100.0, 0.0]);
    sheet.set_value(0, "22k");
    sheet.copy_part(0, [0.0, -500.0], "R2").unwrap();
    sheet.drawing.add_wire(vec![[0.0, 0.0], [0.0, 300.0]]);
    sheet.drawing.add_label([0.0, 300.0], "NEW");
    let out = dir.path().join("edited.SchDoc");
    std::fs::write(
        &out,
        trace_altium::write_schdoc(&sheet, Some(&path)).unwrap(),
    )
    .unwrap();
    let back = trace_altium::read_schdoc(&out).unwrap();
    let des: Vec<_> = back
        .symbols
        .iter()
        .map(|s| (s.designator.as_str(), s.value.as_deref()))
        .collect();
    assert_eq!(des, [("R1", Some("22k")), ("R2", Some("22k"))]);
    assert!(back.net_labels.contains(&"NEW".to_string()));
    let pins = |s: &trace_core::Sheet| {
        let mut v: Vec<_> = s
            .drawing
            .pins
            .iter()
            .map(|p| (p.at[0] as i64, p.at[1] as i64))
            .collect();
        v.sort();
        v
    };
    assert_eq!(pins(&back), pins(&sheet));
    assert_eq!(back.drawing.wires.len(), sheet.drawing.wires.len());
    // The moved part keeps its hidden parameter.
    let text = String::from_utf8_lossy(&std::fs::read(&out).unwrap()).into_owned();
    assert!(text.contains("NAME=Tolerance"));
}
