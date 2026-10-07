//! Round-trip and shape coverage for XMP packet handling.

use std::assert_matches;
use std::path::PathBuf;

use xmp::{LangAlt, Value, X_DEFAULT, Xmp, XmpError};

fn template() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../xmp/hypergryph.xml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn reads_the_shipped_template() {
    let xmp = Xmp::parse(&template()).expect("template parses");

    assert_eq!(
        xmp.get_text("xmp", "CreatorTool"),
        Some("EIHRTeam Media Provenance Service")
    );
    assert_eq!(xmp.get_text("dc", "format"), Some("image/png"));

    // The three timestamps ship as placeholders; parsing must not invent values.
    assert_eq!(xmp.get_text("xmp", "CreateDate"), Some("时间戳-上传时间"));
    assert_eq!(xmp.get_text("xmp", "ModifyDate"), Some("时间戳-同上"));

    match xmp.get("dc", "creator") {
        Some(Value::Seq(items)) => {
            assert_eq!(items, &["Shanghai Hypergryph Network Technology Co., Ltd."])
        }
        other => panic!("dc:creator should be an rdf:Seq, got {other:?}"),
    }

    assert_eq!(
        xmp.get_text("photoshop", "Credit"),
        Some("SKLAND Endfield Wiki Editorial Team")
    );

    match xmp.get("dc", "rights") {
        Some(Value::LangAlt(alt)) => assert!(
            alt.get(X_DEFAULT)
                .unwrap()
                .starts_with("Copyright © Shanghai Hypergryph"),
            "unexpected x-default: {alt:?}"
        ),
        other => panic!("dc:rights should be a language alternative, got {other:?}"),
    }

    assert_eq!(
        xmp.get("xmpRights", "Marked").and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn trims_whitespace_around_simple_values() {
    // The template puts these on their own line, and per the XMP rules that
    // newline is part of the value unless it is trimmed.
    let xmp = Xmp::parse(&template()).unwrap();
    assert_eq!(
        xmp.get_text("photoshop", "Source"),
        Some("Shanghai Hypergryph Network Technology Co., Ltd.")
    );
    assert_eq!(
        xmp.get_text("xmpRights", "WebStatement"),
        Some("https://endfield.gryphline.com/en-us/news/4497")
    );
}

#[test]
fn round_trips_without_loss() {
    let original = Xmp::parse(&template()).unwrap();
    let reparsed = Xmp::parse(&original.to_packet()).unwrap();
    assert_eq!(original, reparsed, "packet did not survive a round trip");

    // And it is stable: a second pass changes nothing.
    assert_eq!(reparsed.to_packet(), original.to_packet());
}

#[test]
fn packet_carries_the_wrapper() {
    let packet = Xmp::parse(&template()).unwrap().to_packet();
    assert!(packet.starts_with("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>"));
    assert!(packet.ends_with("<?xpacket end=\"w\"?>"));
}

#[test]
fn drops_a_declaration_the_packet_does_not_use() {
    // Serialisation declares only the namespaces the properties actually need.
    // The shipped template used to show this with an unused `plus` declaration;
    // it now uses every namespace it declares, so the negative case needs a
    // packet built for it rather than losing the coverage.
    let packet = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"
          xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/">
          <dc:source>x</dc:source>
        </rdf:Description>
    </rdf:RDF>"#;
    let out = Xmp::parse(packet).unwrap().to_packet();
    assert!(
        !out.contains("ns.adobe.com/xap/1.0/mm/"),
        "declared an unused namespace: {out}"
    );
}

#[test]
fn reads_attribute_shorthand() {
    // XMP allows simple properties as rdf:Description attributes; every Adobe
    // tool emits this form for some fields.
    let packet = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
      <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about=""
          xmlns:dc="http://purl.org/dc/elements/1.1/"
          dc:format="image/jpeg"/>
      </rdf:RDF>
    </x:xmpmeta>"#;
    let xmp = Xmp::parse(packet).unwrap();
    assert_eq!(xmp.get_text("dc", "format"), Some("image/jpeg"));
}

#[test]
fn normalises_shorthand_to_element_form() {
    let packet = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about=""
          xmlns:dc="http://purl.org/dc/elements/1.1/"
          dc:format="image/jpeg"/>
    </rdf:RDF>"#;
    let out = Xmp::parse(packet).unwrap().to_xml();
    assert!(out.contains("<dc:format>image/jpeg</dc:format>"), "{out}");
}

#[test]
fn handles_uri_values_via_rdf_resource() {
    let packet = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about=""
          xmlns:xmpRights="http://ns.adobe.com/xap/1.0/rights/">
          <xmpRights:WebStatement rdf:resource="https://example.test/terms"/>
        </rdf:Description>
    </rdf:RDF>"#;
    let xmp = Xmp::parse(packet).unwrap();
    assert_eq!(
        xmp.get_text("xmpRights", "WebStatement"),
        Some("https://example.test/terms")
    );
}

#[test]
fn keeps_unmodelled_structures_verbatim() {
    // A Photoshop export carries structs this model does not cover. Dropping
    // them would silently strip provenance data.
    let packet = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about=""
          xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"
          xmlns:stRef="http://ns.adobe.com/xap/1.0/sType/ResourceRef#">
          <xmpMM:DerivedFrom rdf:parseType="Resource">
            <stRef:instanceID>xmp.iid:abc</stRef:instanceID>
          </xmpMM:DerivedFrom>
        </rdf:Description>
    </rdf:RDF>"#;
    let xmp = Xmp::parse(packet).unwrap();
    assert!(
        matches!(xmp.get("xmpMM", "DerivedFrom"), Some(Value::Raw(_))),
        "expected the struct to be preserved, got {:?}",
        xmp.get("xmpMM", "DerivedFrom")
    );

    let out = xmp.to_xml();
    assert!(out.contains("xmp.iid:abc"), "nested value lost:\n{out}");
    assert!(
        out.contains("rdf:parseType=\"Resource\""),
        "shape lost:\n{out}"
    );
}

#[test]
fn escapes_text_and_attribute_values() {
    let mut xmp = Xmp::new();
    xmp.set_text("dc", "source", "A & B <tag> \"quoted\"")
        .unwrap();
    let mut rights = LangAlt::new();
    rights.set(X_DEFAULT, "5 > 3 & 2 < 4");
    xmp.set_lang_alt("dc", "rights", rights).unwrap();

    let reparsed = Xmp::parse(&xmp.to_packet()).unwrap();
    assert_eq!(
        reparsed.get_text("dc", "source"),
        Some("A & B <tag> \"quoted\"")
    );
    match reparsed.get("dc", "rights") {
        Some(Value::LangAlt(alt)) => assert_eq!(alt.get(X_DEFAULT), Some("5 > 3 & 2 < 4")),
        other => panic!("expected a language alternative, got {other:?}"),
    }
}

#[test]
fn x_default_stays_first() {
    let mut alt = LangAlt::new();
    alt.set("ja", "日本語");
    alt.set(X_DEFAULT, "English");
    alt.set("zh", "中文");
    assert_eq!(
        alt.iter().map(|(l, _)| l).collect::<Vec<_>>(),
        vec![X_DEFAULT, "ja", "zh"]
    );
    assert_eq!(alt.default_text(), Some("English"));
}

#[test]
fn rejects_an_unknown_prefix() {
    let mut xmp = Xmp::new();
    assert_matches!(
        xmp.set_text("nope", "Thing", "x"),
        Err(XmpError::UnknownPrefix(_))
    );
    xmp.register_namespace("nope", "https://example.test/ns#")
        .unwrap();
    assert!(xmp.set_text("nope", "Thing", "x").is_ok());
    // A private namespace has to be declared in the output, or the packet is
    // unreadable by anything else.
    assert!(
        xmp.to_xml()
            .contains("xmlns:nope=\"https://example.test/ns#\"")
    );
}

#[test]
fn setting_a_property_keeps_its_position() {
    let mut xmp = Xmp::parse(&template()).unwrap();
    let before: Vec<String> = xmp.properties().iter().map(|p| p.name.clone()).collect();
    xmp.set_text("xmp", "CreatorTool", "changed").unwrap();
    let after: Vec<String> = xmp.properties().iter().map(|p| p.name.clone()).collect();
    assert_eq!(before, after);
    assert_eq!(xmp.get_text("xmp", "CreatorTool"), Some("changed"));
}

#[test]
fn an_empty_packet_is_not_mistaken_for_xmp() {
    assert!(Xmp::parse("<not-xmp/>").is_err());
    assert!(!xmp::looks_like_xmp("<not-xmp/>"));
    assert!(xmp::looks_like_xmp(&template()));
}

#[test]
fn resolves_character_references() {
    // quick-xml surfaces entities as their own events, so a parser that only
    // handles text events loses these characters silently.
    let packet = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">
          <dc:source>caf&#233; &#x2014; r&#233;sum&#233;</dc:source>
        </rdf:Description>
    </rdf:RDF>"#;
    let xmp = Xmp::parse(packet).unwrap();
    assert_eq!(xmp.get_text("dc", "source"), Some("café — résumé"));
}

#[test]
fn refuses_an_entity_it_cannot_resolve() {
    // Better to fail than to quietly drop characters out of a rights statement.
    let packet = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">
          <dc:source>a &nbsp; b</dc:source>
        </rdf:Description>
    </rdf:RDF>"#;
    assert_matches!(Xmp::parse(packet), Err(XmpError::Xml(_)));
}

#[test]
fn refuses_a_packet_that_relabels_a_known_namespace() {
    // Otherwise a packet could bind `dc` to an attacker's URI and have us write
    // properties there, or relabel `rdf` so its elements read as plumbing.
    let hijacked = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:dc="https://evil.test/dc/">
          <dc:source>x</dc:source>
        </rdf:Description>
    </rdf:RDF>"#;
    assert_matches!(
        Xmp::parse(hijacked),
        Err(XmpError::NamespaceConflict { .. })
    );

    let relabelled = r#"<rdf:RDF xmlns:rdf="https://evil.test/rdf#">
        <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"/>
    </rdf:RDF>"#;
    assert_matches!(
        Xmp::parse(relabelled),
        Err(XmpError::NamespaceConflict { .. })
    );

    // Redefining a prefix to the same URI it already has is ordinary and fine.
    let harmless = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">
          <dc:source>x</dc:source>
        </rdf:Description>
    </rdf:RDF>"#;
    assert!(Xmp::parse(harmless).is_ok());
}

#[test]
fn refuses_absurdly_deep_nesting() {
    // The walks that follow parsing are recursive, so depth is bounded where the
    // tree is built.
    let depth = xmp::model::MAX_DEPTH + 10;
    let mut packet =
        String::from("<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">");
    for _ in 0..depth {
        packet.push_str("<a>");
    }
    for _ in 0..depth {
        packet.push_str("</a>");
    }
    packet.push_str("</rdf:RDF>");
    assert_matches!(Xmp::parse(&packet), Err(XmpError::TooDeep(_)));
}

#[test]
fn refuses_names_that_cannot_go_into_xml() {
    let mut xmp = Xmp::new();
    // A name reaching the packet unescaped is an injection point, so it is
    // rejected where it is set rather than at serialisation.
    for bad in ["bad name", "a>b", "1leading", "", "with\"quote"] {
        assert!(
            matches!(xmp.set_text("dc", bad, "v"), Err(XmpError::IllegalName(_))),
            "{bad:?} should have been rejected"
        );
    }
    assert!(xmp.set_text("dc", "ok-name.1", "v").is_ok());
}

#[test]
fn refuses_characters_that_cannot_go_into_xml() {
    let mut xmp = Xmp::new();
    assert_matches!(
        xmp.set_text("dc", "source", "bell \u{0007} here"),
        Err(XmpError::IllegalCharacters)
    );
    // Tab, newline and carriage return are the exceptions XML allows.
    assert!(xmp.set_text("dc", "source", "tab\there\nnewline").is_ok());
}

#[test]
fn refuses_a_character_reference_xml_cannot_represent() {
    let packet = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">
          <dc:source>a&#1;b</dc:source>
        </rdf:Description>
    </rdf:RDF>"#;
    assert!(Xmp::parse(packet).is_err());
}

#[test]
fn properties_are_indented_one_level_under_their_description() {
    // Output formatting is not cosmetic here: these packets get read, diffed and
    // hand-edited. Deeply over-indented properties were the giveaway that the
    // nesting was computed against the wrong level.
    let xml = Xmp::parse(&template()).unwrap().to_xml();
    let mut lines = xml.lines();
    let description = lines.find(|l| l.contains("<rdf:Description")).unwrap();
    let description_indent = description.len() - description.trim_start().len();
    let property = lines.find(|l| l.contains("<xmp:CreatorTool>")).unwrap();
    let property_indent = property.len() - property.trim_start().len();

    assert_eq!(
        property_indent,
        description_indent + 2,
        "properties should sit exactly one level inside the description:\n{xml}"
    );
}
