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
