use vt100::{Color, Parser};

#[test]
fn shrinking_terminal_keeps_bottom_prompt_and_maps_saved_cursor() {
    let mut parser = Parser::new(44, 120, 100);
    let wrapped_output = vec![b'x'; 121];
    parser.process(&wrapped_output);
    assert!(parser.screen().row_wrapped(0));

    parser.process(b"\x1b[1;1Hheader");
    parser.process(b"\x1b[43;8H\x1b7");
    parser.process(b"\x1b[44;1H\x1b[38;5;220mP123ABC>");
    parser.process(b"\x1b[44;9H");

    parser.screen_mut().set_size(31, 120);

    assert_eq!(parser.screen().cursor_position(), (30, 8));
    let prompt = parser
        .screen()
        .cell(30, 0)
        .expect("bottom prompt remains visible");
    assert_eq!(prompt.contents(), "P");
    assert_eq!(prompt.fgcolor(), Color::Idx(220));
    assert_eq!(parser.screen().scrollback(), 0);

    parser.screen_mut().set_size(31, 80);
    assert_eq!(
        parser
            .screen()
            .cell(30, 0)
            .expect("prompt survives narrow resize")
            .fgcolor(),
        Color::Idx(220)
    );
    parser.screen_mut().set_size(31, 120);

    parser.process(b"\x1b8");
    assert_eq!(parser.screen().cursor_position(), (29, 7));

    parser.screen_mut().set_scrollback(13);
    assert_eq!(
        parser
            .screen()
            .cell(0, 0)
            .expect("trimmed row is retained in scrollback")
            .contents(),
        "h"
    );
    assert!(parser.screen().row_wrapped(0));
    parser.screen_mut().set_scrollback(0);

    parser.process(b"\x1b[31;9H");
    parser.screen_mut().set_size(44, 120);
    #[cfg(windows)]
    {
        let retained_prompt = parser
            .screen()
            .cell(30, 0)
            .expect("ConPTY growth retains the visible prompt row");
        assert_eq!(retained_prompt.contents(), "P");
        assert_eq!(retained_prompt.fgcolor(), Color::Idx(220));
        assert_eq!(parser.screen().cell(43, 0).unwrap().contents(), "");
        assert_eq!(parser.screen().cursor_position(), (30, 8));
        parser.screen_mut().set_scrollback(13);
        assert_eq!(parser.screen().cell(0, 0).unwrap().contents(), "h");
        assert!(parser.screen().row_wrapped(0));
    }

    #[cfg(not(windows))]
    {
        let restored_prompt = parser
            .screen()
            .cell(43, 0)
            .expect("Unix growth restores recent history to the viewport");
        assert_eq!(restored_prompt.contents(), "P");
        assert_eq!(restored_prompt.fgcolor(), Color::Idx(220));
        assert_eq!(parser.screen().cursor_position(), (43, 8));
        assert_eq!(parser.screen().cell(0, 0).unwrap().contents(), "h");
        assert!(parser.screen().row_wrapped(0));
    }
}

#[test]
fn resizing_preserves_bottom_rows_on_both_screen_buffers() {
    let mut parser = Parser::new(44, 120, 100);
    parser.process(b"\x1b[44;1H\x1b[38;5;33mNORMAL>");
    parser.process(b"\x1b[44;8H\x1b[?1049h");
    parser.process(b"\x1b[44;1H\x1b[38;5;196mALT>");
    parser.process(b"\x1b[44;5H");

    parser.screen_mut().set_size(31, 120);

    assert!(parser.screen().alternate_screen());
    let alternate_prompt = parser
        .screen()
        .cell(30, 0)
        .expect("alternate bottom prompt remains visible");
    assert_eq!(alternate_prompt.contents(), "A");
    assert_eq!(alternate_prompt.fgcolor(), Color::Idx(196));

    parser.process(b"\x1b[?1049l");
    assert!(!parser.screen().alternate_screen());
    let normal_prompt = parser
        .screen()
        .cell(30, 0)
        .expect("normal bottom prompt remains visible");
    assert_eq!(normal_prompt.contents(), "N");
    assert_eq!(normal_prompt.fgcolor(), Color::Idx(33));
}

#[test]
fn resizing_resets_custom_scroll_region_to_the_full_viewport() {
    let mut parser = Parser::new(44, 80, 100);
    parser.process(b"\x1b[14;1Hretained");
    parser.process(b"\x1b[10;40r");
    parser.process(b"\x1b[44;1H\x1b[38;5;220mP");
    parser.process(b"\x1b[44;1H");

    parser.screen_mut().set_size(31, 80);

    assert_eq!(
        parser
            .screen()
            .cell(30, 0)
            .expect("bottom prompt")
            .contents(),
        "P"
    );
    parser.process(b"\x1b[31;1H\n");
    assert_eq!(
        parser
            .screen()
            .cell(0, 0)
            .expect("the resized full-screen region scrolls from its top")
            .contents(),
        ""
    );
}

#[test]
fn saved_cursor_row_tracks_resize_in_normal_and_alternate_grids() {
    let mut parser = Parser::new(44, 80, 100);
    parser.process(b"\x1b[6;3H\x1b7");
    parser.process(b"\x1b[44;1H\x1b[?47h");
    parser.process(b"\x1b[6;3H\x1b7");
    parser.process(b"\x1b[44;1H");

    parser.screen_mut().set_size(31, 80);
    parser.process(b"\x1b8");
    assert_eq!(parser.screen().cursor_position(), (0, 2));

    parser.process(b"\x1b[?47l\x1b8");
    assert_eq!(parser.screen().cursor_position(), (0, 2));
    parser.process(b"\x1b[31;1H");
    parser.screen_mut().set_size(44, 80);
    parser.process(b"\x1b8");
    #[cfg(windows)]
    assert_eq!(parser.screen().cursor_position(), (0, 2));
    #[cfg(not(windows))]
    assert_eq!(parser.screen().cursor_position(), (5, 2));

    parser.process(b"\x1b[?47h\x1b8");
    assert_eq!(parser.screen().cursor_position(), (0, 2));
}

#[test]
fn saved_cursor_tracks_conpty_shrink_and_growth_without_history_pullback() {
    let mut parser = Parser::new(44, 80, 100);
    parser.process(b"\x1b[6;3H\x1b7");
    parser.process(b"\x1b[44;1H");

    parser.screen_mut().set_size(31, 80);
    assert_eq!(parser.screen().cursor_position(), (30, 0));
    parser.screen_mut().set_size(44, 80);

    parser.process(b"\x1b8");
    #[cfg(windows)]
    assert_eq!(parser.screen().cursor_position(), (0, 2));
    #[cfg(not(windows))]
    assert_eq!(parser.screen().cursor_position(), (5, 2));
    assert_eq!(parser.screen().scrollback(), 0);
}

#[test]
fn resize_scrollback_cap_rebases_a_saved_cursor_that_lost_its_row() {
    let mut parser = Parser::new(44, 80, 3);
    parser.process(b"\x1b[6;3H\x1b7");
    parser.process(b"\x1b[44;1H");

    parser.screen_mut().set_size(31, 80);
    parser.screen_mut().set_scrollback(13);
    assert_eq!(parser.screen().scrollback(), 3);
    parser.screen_mut().set_scrollback(0);
    parser.process(b"\x1b8");
    assert_eq!(parser.screen().cursor_position(), (0, 2));
}

#[cfg(not(windows))]
#[test]
fn unix_history_pullback_resizes_rows_before_writing_at_the_new_right_edge() {
    let mut parser = Parser::new(10, 40, 100);
    parser.process(b"\x1b[10;1H");

    parser.screen_mut().set_size(8, 20);
    parser.screen_mut().set_size(10, 40);
    parser.process(b"\x1b[1;40HX");

    assert_eq!(
        parser
            .screen()
            .cell(0, 39)
            .expect("restored history row has the resized width")
            .contents(),
        "X"
    );
}

#[cfg(not(windows))]
#[test]
fn unix_history_pullback_preserves_wrap_when_the_row_width_is_unchanged() {
    let mut parser = Parser::new(10, 40, 100);
    parser.process(&[b'x'; 41]);
    parser.process(b"\x1b[10;1H");
    assert!(parser.screen().row_wrapped(0));

    parser.screen_mut().set_size(8, 40);
    parser.screen_mut().set_size(10, 40);

    assert!(parser.screen().row_wrapped(0));
}

#[test]
fn normal_output_scrollback_eviction_preserves_the_xterm_saved_cursor_position() {
    let mut parser = Parser::new(4, 8, 2);
    parser.process(b"\x1b[4;2H\x1b7\n\n\n\x1b8");

    assert_eq!(parser.screen().cursor_position(), (1, 1));
}

#[test]
fn transient_width_fit_during_vertical_shrink_preserves_bottom_prompt_state() {
    let mut parser = Parser::new(44, 120, 1_000);
    parser.process(
        b"\x1b[2J\x1b[H\x1b[1;1H\x1b[38;5;45mTB0AC10\x1b[0m\x1b[2;1H\x1b[38;5;82mMB0AC10\x1b[0m\x1b[44;1H\x1b[38;5;220mPB0AC10>\x1b[0m\x1b[?25h\x1b[44;9H",
    );

    parser.screen_mut().set_size(31, 118);
    parser.screen_mut().set_size(31, 120);

    let prompt = parser
        .screen()
        .cell(30, 0)
        .expect("bottom prompt remains visible after the fit settles");
    assert_eq!(prompt.contents(), "P");
    assert_eq!(prompt.fgcolor(), Color::Idx(220));
    assert_eq!(parser.screen().cursor_position(), (30, 8));

    let snapshot = parser.screen().state_formatted();
    let mut replay = Parser::new(31, 120, 1_000);
    replay.process(&snapshot);
    let replayed_prompt = replay
        .screen()
        .cell(30, 0)
        .expect("formatted frame retains its bottom prompt");
    assert_eq!(replayed_prompt.contents(), "P");
    assert_eq!(replayed_prompt.fgcolor(), Color::Idx(220));
}

#[test]
fn writing_at_a_clipped_wide_glyph_does_not_panic() {
    let mut parser = Parser::new(4, 40, 100);
    parser.process(b"\x1b[1;20H\xe7\x95\x8c");
    parser.screen_mut().set_size(4, 20);

    parser.process(b"\x1b[1;20HX");

    assert_eq!(parser.screen().cell(0, 19).unwrap().contents(), "X");
}
