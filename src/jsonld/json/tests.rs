use super::*;

#[test]
fn inner_spans_leave_out_only_the_quotes_written() {
    let root = |t: &str| parse(t).root.unwrap();
    assert_eq!(root("\"ab\"").inner, Span::new(1, 3));
    // Unclosed at the end of the text: only the opening quote goes.
    assert_eq!(root("\"ab").inner, Span::new(1, 3));
    let o = root("{\"k\": \"wd:");
    let m = &o.as_object().unwrap().members[0];
    assert_eq!(
        (m.key_inner, m.value.inner),
        (Span::new(2, 3), Span::new(7, 10))
    );
    // A key without quotes keeps its whole span.
    let o = root("{abc: 1}");
    assert_eq!(o.as_object().unwrap().members[0].key_inner, Span::new(1, 4));
}

#[test]
fn the_last_duplicate_key_wins() {
    let member = |key: &str, at: u32, v: bool| Member {
        key: key.into(),
        key_span: Span::new(at, at + 3),
        key_inner: Span::new(at + 1, at + 2),
        value: Json {
            value: Value::Bool(v),
            span: Span::new(at + 5, at + 9),
            inner: Span::new(at + 5, at + 9),
        },
    };
    let o = Object {
        members: vec![
            member("a", 1, false),
            member("b", 12, true),
            member("a", 23, true),
        ],
        open: Span::new(0, 1),
        close: Some(Span::new(33, 34)),
    };
    assert_eq!(o.get("a").map(|m| m.key_span.start), Some(23));
    assert!(o.get("c").is_none());
}

/// The errors as `(start, end, message)`.
fn errors(text: &str) -> Vec<(u32, u32, String)> {
    parse(text)
        .errors
        .into_iter()
        .map(|e| (e.span.start, e.span.end, e.message().to_string()))
        .collect()
}

/// The span of the `n`th (from 0) occurrence of `needle` in `text`.
fn at(text: &str, needle: &str, n: usize) -> (u32, u32) {
    let start = text.match_indices(needle).nth(n).expect(needle).0 as u32;
    (start, start + needle.len() as u32)
}

/// The warnings as `(start, end, code, message)`.
fn warnings(text: &str) -> Vec<(u32, u32, &'static str, String)> {
    parse(text)
        .warnings
        .into_iter()
        .map(|e| {
            assert_eq!(e.severity, Severity::Warning);
            (e.span.start, e.span.end, e.code, e.message().to_string())
        })
        .collect()
}

fn one_error(text: &str, needle: &str, n: usize, message: &str) {
    let (s, e) = at(text, needle, n);
    assert_eq!(errors(text), vec![(s, e, message.to_string())], "{text}");
}

fn root(text: &str) -> Json {
    parse(text).root.expect("a root value")
}

fn span(s: (u32, u32)) -> Span {
    Span::new(s.0, s.1)
}

#[test]
fn a_valid_document_keeps_every_span() {
    let text = r#"{
  "@context": {"ex": "http://example.org/"},
  "@id": "ex:a",
  "ex:n": [1, -0.5e+10, 1E3, 0, true, false, null],
  "ex:n": {}
}"#;
    let doc = parse(text);
    assert!(doc.errors.is_empty(), "{:?}", doc.errors);
    let root = doc.root.unwrap();
    assert_eq!(root.span, Span::new(0, text.len() as u32));
    let o = root.as_object().unwrap();
    assert_eq!(o.open, Span::new(0, 1));
    assert_eq!(o.close, Some(span(at(text, "}", 2))));
    let keys: Vec<&str> = o.members.iter().map(|m| &*m.key).collect();
    assert_eq!(
        keys,
        ["@context", "@id", "ex:n", "ex:n"],
        "duplicates kept in order"
    );
    let id = &o.members[1];
    assert_eq!(id.key_span, span(at(text, "\"@id\"", 0)));
    assert_eq!(id.value.span, span(at(text, "\"ex:a\"", 0)));
    assert_eq!(id.value.as_str(), Some("ex:a"));
    let ctx = o.members[0].value.as_object().unwrap();
    assert_eq!(ctx.open, span(at(text, "{", 1)));
    assert_eq!(ctx.close, Some(span(at(text, "}", 0))));
    assert_eq!(
        ctx.get("ex").unwrap().value.as_str(),
        Some("http://example.org/")
    );
    let list = o.members[2].value.as_array().unwrap();
    assert_eq!(list.open, span(at(text, "[", 0)));
    assert_eq!(list.close, Some(span(at(text, "]", 0))));
    let values: Vec<Value> = list.items.iter().map(|j| j.value.clone()).collect();
    assert_eq!(
        values,
        [
            Value::Number("1".into()),
            Value::Number("-0.5e+10".into()),
            Value::Number("1E3".into()),
            Value::Number("0".into()),
            Value::Bool(true),
            Value::Bool(false),
            Value::Null,
        ]
    );
    assert_eq!(list.items[1].span, span(at(text, "-0.5e+10", 0)));
    assert_eq!(list.items[6].span, span(at(text, "null", 0)));
    assert_eq!(o.get("ex:n").unwrap().value.span, span(at(text, "{}", 0)));
}

#[test]
fn strings_decode_escapes_and_unicode() {
    let text = r#"["caf\u00e9 é", "\ud83d\ude00 😀", "\"\\\/\b\f\n\r\t", "", "日本"]"#;
    let doc = parse(text);
    assert!(doc.errors.is_empty(), "{:?}", doc.errors);
    let root = doc.root.unwrap();
    let items: Vec<&str> = root
        .as_array()
        .unwrap()
        .items
        .iter()
        .map(|j| j.as_str().unwrap())
        .collect();
    assert_eq!(
        items,
        ["café é", "😀 😀", "\"\\/\u{8}\u{c}\n\r\t", "", "日本"]
    );
    // Spans are bytes, past multi-byte characters.
    let last = &root.as_array().unwrap().items[4];
    assert_eq!(last.span, span(at(text, "\"日本\"", 0)));
}

#[test]
fn empty_text_has_no_root_and_no_error() {
    for text in ["", "  \n\t ", "\u{feff}"] {
        let doc = parse(text);
        assert!(doc.root.is_none() && doc.errors.is_empty(), "{text:?}");
    }
    let doc = parse("\u{feff}{}");
    assert!(doc.errors.is_empty());
    assert_eq!(doc.root.unwrap().span, Span::new(3, 5));
}

#[test]
fn scalars_at_the_top_level() {
    assert_eq!(root("\"a\"").value, Value::String("a".into()));
    assert_eq!(root(" 42 ").span, Span::new(1, 3));
    assert!(errors(" 42 ").is_empty());
}

#[test]
fn invalid_numbers_are_errors_and_invalid_values() {
    for n in ["01", "1.", ".5", "+1", "-", "1e", "1.5e+", "0x10", "12abc"] {
        let text = format!("[{n}, 2]");
        one_error(&text, n, 0, &format!("Invalid number `{n}`"));
        let r = root(&text);
        let items = &r.as_array().unwrap().items;
        assert_eq!(items[0].value, Value::Invalid);
        assert_eq!(
            items[1].value,
            Value::Number("2".into()),
            "the rest is read"
        );
    }
}

#[test]
fn invalid_escapes_are_errors_at_the_escape() {
    let text = r#"["a\xb", "c"]"#;
    one_error(text, r"\x", 0, "Invalid escape `\\x`");
    assert_eq!(
        root(text).as_array().unwrap().items[0].as_str(),
        Some("axb")
    );

    let text = r#"["\u12G4"]"#;
    one_error(
        text,
        r"\u12",
        0,
        "Invalid `\\u` escape: expected 4 hex digits",
    );
    assert_eq!(
        root(text).as_array().unwrap().items[0].as_str(),
        Some("\u{fffd}G4")
    );

    let text = r#"["\u00"]"#;
    one_error(
        text,
        r"\u00",
        0,
        "Invalid `\\u` escape: expected 4 hex digits",
    );

    let text = "[\"\\é\"]";
    one_error(text, "\\é", 0, "Invalid escape `\\é`");
}

#[test]
fn a_lone_surrogate_escape_is_a_warning_read_as_the_replacement_character() {
    // RFC 8259 allows it and other JSON-LD processors accept it: the
    // text is JSON, so no error, and the value is U+FFFD.
    let text = r#"["a\ud83dz"]"#;
    let (s, e) = at(text, r"\ud83d", 0);
    let msg = "Unpaired surrogate in a `\\u` escape; it is read as U+FFFD";
    assert!(errors(text).is_empty());
    assert_eq!(
        warnings(text),
        vec![(s, e, "json-lone-surrogate", msg.to_string())]
    );
    assert_eq!(
        root(text).as_array().unwrap().items[0].as_str(),
        Some("a\u{fffd}z")
    );

    // A low surrogate alone, a high one followed by another escape, and
    // in a key; a valid pair is none.
    let text = r#"["\ude00", "\ud83d\u0041", {"k\ud800": 1}, "\ud83d\ude00"]"#;
    assert!(errors(text).is_empty());
    let spans: Vec<_> = warnings(text)
        .into_iter()
        .map(|w| (w.0, w.1, w.2))
        .collect();
    let (lo, hi, key) = (
        at(text, r"\ude00", 0),
        at(text, r"\ud83d", 0),
        at(text, r"\ud800", 0),
    );
    assert_eq!(
        spans,
        vec![
            (lo.0, lo.1, "json-lone-surrogate"),
            (hi.0, hi.1, "json-lone-surrogate"),
            (key.0, key.1, "json-lone-surrogate"),
        ]
    );
    assert_eq!(
        root(text).as_array().unwrap().items[1].as_str(),
        Some("\u{fffd}A")
    );
    assert_eq!(
        root(text).as_array().unwrap().items[3].as_str(),
        Some("\u{1f600}")
    );
}

/// The members a map drops are the ones warned about, whatever the size
/// of the object (the small and the large way of finding them).
#[test]
fn the_members_entries_drops_are_the_duplicate_key_warnings() {
    let mut rng = 0x2545_F491_4F6C_DD1Du64;
    let mut next = |n: u64| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng % n
    };
    // Under MAX_ERRORS members, so every warning is there.
    for size in [0, 1, 2, 5, 16, 17, 40, 90] {
        for _ in 0..20 {
            let keys = 1 + next(size as u64 + 1);
            let members: Vec<String> = (0..size)
                .map(|i| format!("\"k{}\": {i}", next(keys)))
                .collect();
            let text = format!("{{{}}}", members.join(", "));
            let doc = parse(&text);
            let o = doc.root.as_ref().unwrap().as_object().unwrap();
            let kept: Vec<Span> = o.entries().iter().map(|m| m.key_span).collect();
            let mut dropped: Vec<Span> = o
                .members
                .iter()
                .map(|m| m.key_span)
                .filter(|s| !kept.contains(s))
                .collect();
            let mut warned: Vec<Span> = doc
                .warnings
                .iter()
                .filter(|w| w.code == "json-duplicate-key")
                .map(|w| w.span)
                .collect();
            dropped.sort_by_key(|s| s.start);
            warned.sort_by_key(|s| s.start);
            assert_eq!(dropped, warned, "{text}");
            let mut distinct: Vec<&str> = o.members.iter().map(|m| &*m.key).collect();
            distinct.sort_unstable();
            distinct.dedup();
            assert_eq!(kept.len(), distinct.len(), "{text}");
        }
    }
}

#[test]
fn a_repeated_key_is_a_warning_at_each_earlier_member() {
    let text = r#"{"a": 1, "b": 2, "a": 3, "a": 4, "c": {"x": 1, "x": 2}}"#;
    assert!(errors(text).is_empty());
    let got: Vec<_> = warnings(text)
        .into_iter()
        .map(|w| (w.0, w.1, w.2))
        .collect();
    let (a0, a1, x0) = (
        at(text, "\"a\"", 0),
        at(text, "\"a\"", 1),
        at(text, "\"x\"", 0),
    );
    assert_eq!(
        got,
        vec![
            (a0.0, a0.1, "json-duplicate-key"),
            (a1.0, a1.1, "json-duplicate-key"),
            (x0.0, x0.1, "json-duplicate-key"),
        ]
    );
    // The last member still wins.
    let o = root(text);
    assert_eq!(
        o.as_object().unwrap().get("a").unwrap().value.span.start,
        at(text, "4", 0).0
    );
    // Keys that differ only by their escapes are the same key.
    let text = r#"{"a": 1, "\u0061": 2}"#;
    assert_eq!(warnings(text).len(), 1);
    // Many members take the hashed path; distinct keys give none.
    let many: String = (0..40)
        .map(|i| format!("\"k{}\": 0", i % 20))
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(warnings(&format!("{{{many}}}")).len(), 20);
    let distinct: String = (0..40)
        .map(|i| format!("\"k{i}\": 0"))
        .collect::<Vec<_>>()
        .join(",");
    assert!(warnings(&format!("{{{distinct}}}")).is_empty());
    // No warning without a repeated key, and none from `tokens`.
    assert!(warnings(r#"{"a": {"a": 1}, "b": [{"a": 1}, {"a": 2}]}"#).is_empty());
}

#[test]
fn warnings_do_not_make_a_document_invalid() {
    for text in [r#"{"a": 1, "a": 2}"#, r#""\ud83d""#] {
        let doc = parse(text);
        assert!(doc.errors.is_empty() && doc.root.is_some() && !doc.warnings.is_empty());
    }
}

#[test]
fn warnings_are_capped_without_a_message() {
    let text = format!(
        "[{}]",
        "\"\\ud800\",".repeat(MAX_ERRORS + 50).trim_end_matches(',')
    );
    let doc = parse(&text);
    assert!(doc.errors.is_empty());
    assert_eq!(doc.warnings.len(), MAX_ERRORS);
}

#[test]
fn control_characters_in_strings() {
    let text = "[\"a\tb\"]";
    one_error(
        text,
        "\t",
        0,
        "Control character in a string: write it as an escape (\\t, \\u0001)",
    );
    assert_eq!(
        root(text).as_array().unwrap().items[0].as_str(),
        Some("a\tb")
    );
}

#[test]
fn a_missing_comma_between_members() {
    let text = r#"{"a": 1 "b": 2, "c": [3 4]}"#;
    let (one, three) = (at(text, "1", 0), at(text, "3", 0));
    assert_eq!(
        errors(text),
        vec![
            (one.0, one.1, "Expected ',' between members".into()),
            (three.0, three.1, "Expected ',' between items".into()),
        ]
    );
    let r = root(text);
    let o = r.as_object().unwrap();
    assert_eq!(o.members.len(), 3, "every member is read");
    assert_eq!(o.get("c").unwrap().value.as_array().unwrap().items.len(), 2);
    assert!(o.close.is_some());
}

#[test]
fn a_trailing_comma() {
    one_error(r#"{"a": 1,}"#, ",", 0, "Trailing comma before '}'");
    one_error("[1, 2, ]", ",", 1, "Trailing comma before ']'");
    assert_eq!(root("[1, 2, ]").as_array().unwrap().items.len(), 2);
}

#[test]
fn extra_commas() {
    one_error("[1,, 2]", ",", 1, "Expected a value before ','");
    one_error("[, 2]", ",", 0, "Expected a value before ','");
    one_error(r#"{, "a": 1}"#, ",", 0, "Unexpected ','");
    assert_eq!(root("[1,, 2]").as_array().unwrap().items.len(), 2);
}

#[test]
fn a_missing_value() {
    let text = r#"{"a": , "b": 1}"#;
    one_error(text, ",", 0, "Expected a value, found ','");
    let r = root(text);
    let o = r.as_object().unwrap();
    let a = &o.get("a").unwrap().value;
    let comma = at(text, ",", 0).0;
    assert_eq!(
        (a.value.clone(), a.span),
        (Value::Invalid, Span::new(comma, comma))
    );
    assert_eq!(o.get("b").unwrap().value.value, Value::Number("1".into()));

    one_error(r#"{"a": }"#, "}", 0, "Expected a value, found '}'");
    let text = r#"{"a":"#;
    let end = text.len() as u32;
    assert_eq!(
        errors(text),
        vec![
            (0, 1, "Unclosed object: expected '}'".into()),
            (
                end,
                end,
                "Expected a value, found the end of the file".into()
            ),
        ]
    );
}

#[test]
fn a_missing_colon() {
    let text = r#"{"a" 1, "b": 2}"#;
    one_error(text, "1", 0, "Expected ':' after the key, found a number");
    let r = root(text);
    let o = r.as_object().unwrap();
    assert_eq!(o.get("a").unwrap().value.value, Value::Number("1".into()));
    assert_eq!(o.members.len(), 2);

    let text = r#"{"a", "b": 2}"#;
    one_error(text, ",", 0, "Expected ':' after the key, found ','");
    let r = root(text);
    let a = &r.as_object().unwrap().members[0];
    assert_eq!(a.value.value, Value::Invalid);
    assert_eq!(a.value.span, Span::new(a.key_span.end, a.key_span.end));
}

#[test]
fn an_unclosed_string_ends_at_its_line() {
    let text = "{\n  \"a\": \"oops,\n  \"b\": 2\n}";
    let s = at(text, "\"oops,", 0);
    assert_eq!(
        errors(text),
        vec![(
            s.0,
            s.1,
            "Unclosed string: expected '\"' before the end of the line".into()
        )]
    );
    let r = root(text);
    let o = r.as_object().unwrap();
    assert_eq!(o.get("a").unwrap().value.as_str(), Some("oops,"));
    assert_eq!(o.get("b").unwrap().value.value, Value::Number("2".into()));
    assert!(o.close.is_some());

    assert_eq!(
        errors("[\"abc"),
        vec![
            (0, 1, "Unclosed array: expected ']'".into()),
            (
                1,
                5,
                "Unclosed string: expected '\"' before the end of the line".into()
            ),
        ]
    );
}

#[test]
fn unclosed_objects_and_arrays_are_reported_at_their_bracket() {
    let text = r#"{"a": [1, 2"#;
    let (o, a) = (at(text, "{", 0), at(text, "[", 0));
    assert_eq!(
        errors(text),
        vec![
            (o.0, o.1, "Unclosed object: expected '}'".into()),
            (a.0, a.1, "Unclosed array: expected ']'".into()),
        ]
    );
    let r = root(text);
    assert_eq!(
        r.span,
        Span::new(0, text.len() as u32),
        "up to its last member"
    );
    let obj = r.as_object().unwrap();
    assert_eq!(obj.close, None);
    let list = obj.get("a").unwrap().value.as_array().unwrap();
    assert_eq!((list.items.len(), list.close), (2, None));

    // A closing bracket of an enclosing container closes the inner one.
    let text = r#"[{"a": 1]"#;
    one_error(text, "{", 0, "Unclosed object: expected '}' before ']'");
    let r = root(text);
    let list = r.as_array().unwrap();
    assert_eq!(list.close, Some(span(at(text, "]", 0))));
    assert_eq!(list.items[0].span, Span::new(1, at(text, "1", 0).1));
    let text = r#"{"a": [1}"#;
    one_error(text, "[", 0, "Unclosed array: expected ']' before '}'");
    assert!(root(text).as_object().unwrap().close.is_some());
}

#[test]
fn stray_tokens_are_skipped() {
    let text = r#"{"a": 1]}"#;
    one_error(text, "]", 0, "Unexpected ']'");
    assert!(root(text).as_object().unwrap().close.is_some());
    let text = "[1}]";
    one_error(text, "}", 0, "Unexpected '}'");
    assert_eq!(root(text).as_array().unwrap().items.len(), 1);
    one_error(r#"{"a": 1:, "b": 2}"#, ":", 1, "Unexpected ':'");
    one_error(r#"{"a": 1 [2]}"#, "[", 0, "Expected ',' or '}', found '['");
    one_error(
        r#"{"a": 1, 2: 3}"#,
        "2",
        0,
        "Object keys must be strings in double quotes",
    );
    one_error(
        r#"{"a": 1, [2]: 3}"#,
        "[",
        0,
        "Expected a key in double quotes, found '['",
    );
    let text = r#"{"a": tru, b: 1}"#;
    let (v, k) = (at(text, "tru", 0), at(text, "b", 0));
    assert_eq!(
        errors(text),
        vec![
            (v.0, v.1, "Expected a value, found `tru`".into()),
            (
                k.0,
                k.1,
                "Object keys must be strings in double quotes".into()
            ),
        ]
    );
    let r = root(text);
    let o = r.as_object().unwrap();
    assert_eq!(o.get("a").unwrap().value.span, span(v));
    assert_eq!(o.get("b").unwrap().value.value, Value::Number("1".into()));
}

#[test]
fn text_after_the_root_is_one_error() {
    let text = "{} xyz {\"a\": }\n\n";
    one_error(
        text,
        "xyz {\"a\": }",
        0,
        "Unexpected text after the top-level value",
    );
    one_error("[]]", "]", 1, "Unexpected text after the top-level value");
    let text = "} {}";
    one_error(text, "} {}", 0, "Expected a JSON value, found '}'");
    assert_eq!(root(text).value, Value::Invalid);
}

#[test]
fn comments_and_single_quotes_are_errors_but_read() {
    let text = "{\n  // note\n  'a': /* x */ 1\n}";
    let (c1, q, c2) = (
        at(text, "// note", 0),
        at(text, "'a'", 0),
        at(text, "/* x */", 0),
    );
    assert_eq!(
        errors(text),
        vec![
            (c1.0, c1.1, "JSON has no comments".into()),
            (q.0, q.1, "Strings use double quotes".into()),
            (c2.0, c2.1, "JSON has no comments".into()),
        ]
    );
    let r = root(text);
    assert_eq!(
        r.as_object().unwrap().get("a").unwrap().value.value,
        Value::Number("1".into())
    );
}

#[test]
fn later_errors_are_still_reported() {
    let text = "{\n  \"a\": 1,,\n  \"b\": [1 2],\n  \"c\": {\"d\" 4},\n  \"e\": 5,\n}";
    let messages: Vec<String> = errors(text).into_iter().map(|e| e.2).collect();
    assert_eq!(
        messages,
        [
            "Unexpected ','",
            "Expected ',' between items",
            "Expected ':' after the key, found a number",
            "Trailing comma before '}'",
        ]
    );
    let r = root(text);
    let keys: Vec<&str> = r
        .as_object()
        .unwrap()
        .members
        .iter()
        .map(|m| &*m.key)
        .collect();
    assert_eq!(keys, ["a", "b", "c", "e"]);
}

#[test]
fn deep_nesting_is_an_error_not_a_stack_overflow() {
    let ok = format!(
        "{}{}",
        "[".repeat(MAX_DEPTH as usize),
        "]".repeat(MAX_DEPTH as usize)
    );
    assert!(errors(&ok).is_empty());

    let n = MAX_DEPTH as usize + 1;
    let deep = format!("{}{}", "[".repeat(n), "]".repeat(n));
    let d = MAX_DEPTH;
    assert_eq!(
        errors(&deep),
        vec![(
            d,
            d + 1,
            format!("Nested more than {MAX_DEPTH} levels deep")
        )]
    );
    let r = root(&deep);
    assert_eq!(r.span, Span::new(0, deep.len() as u32));

    // Unclosed and far deeper: one error per unclosed level up to the
    // limit, capped, and no recursion past the limit.
    let text = "{\"a\":".repeat(200_000);
    let errs = parse(&text).errors;
    assert_eq!(errs.len(), MAX_ERRORS + 1);
}

#[test]
fn errors_are_capped() {
    let text = format!("[{}1]", ",".repeat(500));
    let errs = parse(&text).errors;
    assert_eq!(errs.len(), MAX_ERRORS + 1);
    assert!(
        errs[MAX_ERRORS]
            .message()
            .starts_with("More than 100 syntax errors")
    );
}

#[test]
fn the_cap_message_sits_at_the_end_of_the_reported_range() {
    // Errors that are reported at an earlier place than the text read
    // (a trailing comma, an unclosed opener) must not pull the message
    // into the middle of the list: it is empty, after every error, and
    // not past the text.
    let texts = [
        format!("[{}1]", ",".repeat(500)),
        // Each trailing comma is reported when its object closes.
        format!("[{}{{}}]", "{\"a\": 1,},\n".repeat(300)),
        format!("[{}]", "[1,],".repeat(300)),
        "[".repeat(200) + &"1 2 ".repeat(300),
        format!("[{}\"é", "x ".repeat(300)),
    ];
    for text in texts {
        let errs = parse(&text).errors;
        assert_eq!(
            errs.len(),
            MAX_ERRORS + 1,
            "{}",
            &text[..20.min(text.len())]
        );
        let cap = errs.last().unwrap();
        assert!(cap.message().starts_with("More than 100 syntax errors"));
        assert_eq!(cap.span.start, cap.span.end);
        assert!(cap.span.end as usize <= text.len());
        assert!(text.is_char_boundary(cap.span.end as usize));
        let before = errs[..MAX_ERRORS].iter().map(|e| e.span.end).max().unwrap();
        assert!(
            cap.span.start >= before,
            "{cap:?} before the error ending at {before}"
        );
        assert!(errs.windows(2).all(|w| w[0].span.start <= w[1].span.start));
    }
}

#[test]
fn a_backslash_does_not_take_a_line_break_with_it() {
    // The lexer ends an unclosed string at the break; the editor scans
    // must see the same end.
    for text in [
        "\"a\\\nb\"",
        "\"a\\\r\nb\"",
        "\"a\\\"b",
        "\"a\\",
        "\"a\\\\\" c",
        "\"é\\\n\"x\"",
        "\"abc",
        "\"\"",
    ] {
        let first = tokens(text).into_iter().next().unwrap();
        assert_eq!(first.kind, TokenKind::String, "{text:?}");
        let (end, closed) = string_end(text.as_bytes(), 0);
        let want_end = first.span.end as usize;
        assert_eq!(
            if closed { end + 1 } else { end },
            want_end,
            "{text:?}: the string_end and the token disagree"
        );
        assert_eq!(
            closed,
            text[..want_end].len() > 1 && text[..want_end].ends_with('"') && {
                // Closed unless the quote is escaped away or is the opener.
                parse(text)
                    .errors
                    .iter()
                    .all(|e| !e.message().starts_with("Unclosed string"))
            },
            "{text:?}"
        );
    }
}

#[test]
fn tokens_for_highlighting() {
    let text = "{\"a\": [1, true, x]} // c";
    let kinds: Vec<(TokenKind, &str)> = tokens(text)
        .into_iter()
        .map(|t| (t.kind, &text[t.span.start as usize..t.span.end as usize]))
        .collect();
    use TokenKind::*;
    assert_eq!(
        kinds,
        [
            (LBrace, "{"),
            (String, "\"a\""),
            (Colon, ":"),
            (LBracket, "["),
            (Number, "1"),
            (Comma, ","),
            (Literal, "true"),
            (Comma, ","),
            (Invalid, "x"),
            (RBracket, "]"),
            (RBrace, "}"),
            (Comment, "// c"),
        ]
    );
}

/// The benchmark for the parser, run with `cargo test --release jsonld`:
/// a 30,000-line JSON-LD file against a 30,000-line Turtle file with the
/// same data. Prints both times; fails when JSON is over twice as slow.
#[cfg(not(debug_assertions))]
#[test]
fn timing_parses_30000_lines_about_as_fast_as_turtle() {
    use std::time::{Duration, Instant};
    let mut json =
        String::from("{\n  \"@context\": {\"ex\": \"http://example.org/\"},\n  \"@graph\": [\n");
    let mut ttl = String::from("@prefix ex: <http://example.org/> .\n");
    let items = 30_000 / 7;
    for i in 0..items {
        let sep = if i + 1 < items { "," } else { "" };
        json.push_str(&format!(
                "    {{\n      \"@id\": \"ex:item{i}\",\n      \"@type\": \"ex:Thing\",\n      \"ex:name\": \"Item {i} \\u00e9 caf\u{e9}\",\n      \"ex:value\": {i}.5,\n      \"ex:tags\": [\"a\", \"b\"]\n    }}{sep}\n"
            ));
        ttl.push_str(&format!(
                "ex:item{i}\n  a ex:Thing ;\n  ex:name \"Item {i} \\u00e9 caf\u{e9}\" ;\n  ex:value {i}.5 ;\n  ex:tags \"a\" ,\n    \"b\" .\n\n"
            ));
    }
    json.push_str("  ]\n}\n");
    let best = |f: &dyn Fn()| {
        (0..7)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed()
            })
            .min()
            .unwrap_or(Duration::ZERO)
    };
    let doc = parse(&json);
    assert!(doc.errors.is_empty(), "{:?}", &doc.errors[..1]);
    let prefixes = crate::syntax::PrefixMap::default();
    let p = crate::syntax::parse(&ttl, &prefixes, crate::syntax::Syntax::Turtle);
    assert!(p.errors.is_empty());
    let j = best(&|| drop(parse(&json)));
    let t = best(&|| {
        drop(crate::syntax::parse(
            &ttl,
            &prefixes,
            crate::syntax::Syntax::Turtle,
        ))
    });
    eprintln!(
        "json::parse {} lines ({} KB): {j:?}; Turtle {} lines ({} KB): {t:?}",
        json.lines().count(),
        json.len() / 1024,
        ttl.lines().count(),
        ttl.len() / 1024,
    );
    assert!(
        j <= t * 2 + Duration::from_millis(2),
        "JSON {j:?} vs Turtle {t:?}"
    );
}
