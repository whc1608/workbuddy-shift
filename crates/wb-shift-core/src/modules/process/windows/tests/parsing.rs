    #[test]
    fn self_image_names_are_detected() {
        assert!(is_self_image_name("workbuddy-shift"));
        assert!(is_self_image_name("workbuddy-shift.exe"));
        assert!(is_self_image_name("WORKBUDDY-SHIFT-DESKTOP.EXE"));
        assert!(is_self_image_name("workbuddy-shift-desktop"));
        assert!(is_self_image_name(r"C:\apps\workbuddy-shift.exe"));
        assert!(!is_self_image_name("WorkBuddy.exe"));
        assert!(!is_self_image_name("WorkBuddy"));
        assert!(!is_self_image_name("CodeBuddy.exe"));
    }

    #[test]
    fn workbuddy_image_name_is_exact_not_substring() {
        assert!(is_workbuddy_image_name("WorkBuddy.exe", WbVariant::Cn));
        assert!(is_workbuddy_image_name("workbuddy", WbVariant::Cn));
        assert!(!is_workbuddy_image_name("CodeBuddy.exe", WbVariant::Cn));
        assert!(!is_workbuddy_image_name("CODEBUDDY", WbVariant::Cn));
        assert!(!is_workbuddy_image_name(
            "workbuddy-shift.exe",
            WbVariant::Cn
        ));
        assert!(!is_workbuddy_image_name("workbuddy-shift", WbVariant::Cn));
        assert!(!is_workbuddy_image_name(
            "WorkBuddy Helper.exe",
            WbVariant::Cn
        ));
        assert!(!is_workbuddy_image_name("MyWorkBuddy.exe", WbVariant::Cn));
        assert!(is_workbuddy_exe_file_name(
            Path::new("WorkBuddy.exe"),
            WbVariant::Cn
        ));
        assert!(is_workbuddy_exe_file_name(
            Path::new(r"D:\Users\Zhou\AppData\Local\Programs\WorkBuddy\WorkBuddy.exe"),
            WbVariant::Cn
        ));
        assert!(!is_workbuddy_exe_file_name(
            Path::new("workbuddy-shift.exe"),
            WbVariant::Cn
        ));
        assert!(!is_workbuddy_exe_file_name(
            Path::new("Uninstall.exe"),
            WbVariant::Cn
        ));
    }

    /// 两档位映像名必须互斥：跨档位命中会误杀另一版客户端。
    #[test]
    fn variant_image_names_never_cross_match() {
        assert!(is_workbuddy_image_name("WorkBuddyAI.exe", WbVariant::Ai));
        assert!(is_workbuddy_image_name("workbuddyai", WbVariant::Ai));
        assert!(
            !is_workbuddy_image_name("WorkBuddy.exe", WbVariant::Ai),
            "国内版主程序不得命中国际版判定"
        );
        assert!(!is_workbuddy_image_name("CodeBuddy.exe", WbVariant::Ai));

        assert!(
            !is_workbuddy_image_name("WorkBuddyAI.exe", WbVariant::Cn),
            "国际版主程序不得命中国内版判定"
        );
        assert!(!is_workbuddy_image_name("workbuddyai", WbVariant::Cn));

        for variant in WbVariant::ALL {
            for name in [
                "workbuddy-shift.exe",
                "workbuddy-shift",
                "WORKBUDDY-SHIFT-DESKTOP.EXE",
                "WorkBuddySwitch.exe",
            ] {
                assert!(
                    !is_workbuddy_image_name(name, variant),
                    "{name} 不得命中 {variant:?}"
                );
            }
        }

        assert_eq!(
            windows_exe_names(WbVariant::Ai),
            vec!["WorkBuddyAI.exe".to_string()]
        );
        assert_eq!(
            windows_exe_names(WbVariant::Cn),
            vec!["WorkBuddy.exe".to_string()]
        );
    }

    #[test]
    fn ai_process_rows_do_not_swallow_cn_processes() {
        let stdout = "\
1001|WorkBuddyAI|D:\\Users\\Zhou\\AppData\\Local\\Programs\\WorkBuddyAI\\WorkBuddyAI.exe
1002|WorkBuddy|D:\\Users\\Zhou\\AppData\\Local\\Programs\\WorkBuddy\\WorkBuddy.exe
1003|workbuddy-shift|C:\\apps\\workbuddy-shift.exe
";
        let rows = parse_windows_process_rows(stdout);
        let ai: Vec<u32> = filter_windows_workbuddy_rows(&rows, WbVariant::Ai)
            .iter()
            .map(|r| r.pid)
            .collect();
        let cn: Vec<u32> = filter_windows_workbuddy_rows(&rows, WbVariant::Cn)
            .iter()
            .map(|r| r.pid)
            .collect();
        assert_eq!(ai, vec![1001]);
        assert_eq!(cn, vec![1002]);
    }

    #[test]
    fn registry_probe_script_is_variant_scoped() {
        let ai = windows_registry_probe_script(WbVariant::Ai);
        assert!(ai.contains("@('WorkBuddyAI.exe')"));
        assert!(ai.contains("-notmatch 'WorkBuddyAI'"));
        assert!(
            ai.contains("workbuddy-shift|workbuddy-shift-desktop"),
            "自排除保留"
        );

        let cn = windows_registry_probe_script(WbVariant::Cn);
        assert!(cn.contains("@('WorkBuddy.exe')"));
        assert!(cn.contains("-notmatch 'WorkBuddy'"));
        assert!(!cn.contains("CodeBuddy.exe"));
    }

    #[test]
    fn parse_display_icon_strips_quotes_and_index() {
        assert_eq!(
            parse_windows_display_icon(
                r#""D:\Users\Zhou\AppData\Local\Programs\WorkBuddy\WorkBuddy.exe,0""#
            ),
            Some(r"D:\Users\Zhou\AppData\Local\Programs\WorkBuddy\WorkBuddy.exe".to_string())
        );
        assert_eq!(
            parse_windows_display_icon(
                r#""D:\Users\Zhou\AppData\Local\Programs\WorkBuddy\WorkBuddy.exe",0"#
            ),
            Some(r"D:\Users\Zhou\AppData\Local\Programs\WorkBuddy\WorkBuddy.exe".to_string())
        );
        assert_eq!(
            parse_windows_display_icon(r"C:\Program Files\WorkBuddy\WorkBuddy.exe"),
            Some(r"C:\Program Files\WorkBuddy\WorkBuddy.exe".to_string())
        );
        assert_eq!(parse_windows_display_icon("  "), None);
    }

    #[test]
    fn fallback_candidates_include_d_drive_for_zhou() {
        let cands = windows_fallback_exe_candidates(
            Some(r"C:\Users\Zhou\AppData\Local"),
            Some(r"C:\Program Files"),
            Some(r"C:\Program Files (x86)"),
            Some("Zhou"),
            &['C', 'D'],
            WbVariant::Cn,
        );
        let want = r"D:\Users\Zhou\AppData\Local\Programs\WorkBuddy\WorkBuddy.exe";
        assert!(
            cands.iter().any(|p| p.to_string_lossy() == want),
            "missing {want} in {cands:?}"
        );
        let d_pf = r"D:\Program Files\WorkBuddy\WorkBuddy.exe";
        assert!(
            cands.iter().any(|p| p.to_string_lossy() == d_pf),
            "missing {d_pf} in {cands:?}"
        );
        let local_default = r"C:\Users\Zhou\AppData\Local\Programs\WorkBuddy\WorkBuddy.exe";
        assert!(
            cands.iter().any(|p| p.to_string_lossy() == local_default),
            "missing {local_default} in {cands:?}"
        );
    }

    #[test]
    fn fallback_candidates_include_drive_root_install() {
        // 便携版：D:\WorkBuddy\WorkBuddy.exe（既不在 Program Files，也不在 LOCALAPPDATA）。
        let cands = windows_fallback_exe_candidates(
            Some(r"C:\Users\Zhou\AppData\Local"),
            Some(r"C:\Program Files"),
            None,
            Some("Zhou"),
            &['C', 'D'],
            WbVariant::Cn,
        );
        let want = r"D:\WorkBuddy\WorkBuddy.exe";
        assert!(
            cands.iter().any(|p| p.to_string_lossy() == want),
            "missing {want} in {cands:?}"
        );
    }

    #[test]
    fn ai_fallback_candidates_stay_within_variant() {
        let cands = windows_fallback_exe_candidates(
            Some(r"C:\Users\Zhou\AppData\Local"),
            None,
            None,
            Some("Zhou"),
            &['D'],
            WbVariant::Ai,
        );
        let s: Vec<String> = cands
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert!(
            s.contains(
                &r"D:\Users\Zhou\AppData\Local\Programs\WorkBuddyAI\WorkBuddyAI.exe".to_string()
            ),
            "missing AI candidate in {s:?}"
        );
        assert!(
            s.iter().all(|p| !p.contains("WorkBuddy.exe")),
            "国际版候选不得出现国内版程序名: {s:?}"
        );
    }

    #[test]
    fn process_rows_drop_self_and_crashpad() {
        let stdout = "\
1001|workbuddy-shift|C:\\apps\\workbuddy-shift.exe
1002|workbuddy-shift|
1003|WorkBuddy|D:\\Users\\Zhou\\AppData\\Local\\Programs\\WorkBuddy\\WorkBuddy.exe
1004|crashpad_handler|C:\\x\\crashpad_handler.exe
1005|CodeBuddy|
1006|WorkBuddy Helper|
";
        let kept =
            filter_windows_workbuddy_rows(&parse_windows_process_rows(stdout), WbVariant::Cn);
        let pids: Vec<u32> = kept.iter().map(|r| r.pid).collect();
        assert_eq!(pids, vec![1003]);
    }

    #[test]
    fn only_switcher_process_is_not_workbuddy_running() {
        let stdout = "4400|workbuddy-shift.exe|C:\\Users\\Zhou\\AppData\\Local\\Programs\\workbuddy-shift\\workbuddy-shift.exe\n";
        let kept =
            filter_windows_workbuddy_rows(&parse_windows_process_rows(stdout), WbVariant::Cn);
        assert!(kept.is_empty());
        assert!(
            filter_windows_workbuddy_rows(&parse_windows_process_rows(stdout), WbVariant::Ai)
                .is_empty()
        );

        let csv = "\
\"workbuddy-shift.exe\",\"4400\",\"Console\",\"1\",\"10,000 K\"
\"workbuddy-shift.exe\",\"4401\",\"Console\",\"1\",\"8,000 K\"
";
        let kept = filter_windows_workbuddy_rows(&parse_tasklist_csv(csv), WbVariant::Cn);
        assert!(kept.is_empty());
    }

    #[test]
    fn tasklist_csv_keeps_exact_workbuddy_image() {
        let csv = "\
\"WorkBuddy.exe\",\"1234\",\"Console\",\"1\",\"50,123 K\"
\"workbuddy-shift.exe\",\"4400\",\"Console\",\"1\",\"10,000 K\"
INFO: No tasks are running which match the specified criteria.
";
        let kept = filter_windows_workbuddy_rows(&parse_tasklist_csv(csv), WbVariant::Cn);
        assert_eq!(kept.iter().map(|r| r.pid).collect::<Vec<_>>(), vec![1234]);
    }
