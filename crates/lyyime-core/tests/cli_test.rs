//! lyyime-cli 二进制端到端测试:逐键 JSON 输出、--cands 明细、参数错误码。
//! 全部用例把 HOME 重定向到临时目录,绝不读写真实用户数据。

mod common;

use common::{fixtures, TempDir};
use std::process::Command;

/// 启动 lyyime-cli,HOME 指向独立临时目录(避免写真实 ~/.local/share)。
fn cli(td: &TempDir) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lyyime-cli"));
    cmd.env("HOME", &td.path);
    cmd
}

#[test]
fn cli_replay_逐键输出效果json() {
    let td = TempDir::new();
    let out = cli(&td)
        .args([
            "--data-dir",
            fixtures().to_str().unwrap(),
            "replay",
            "nihao<space>",
        ])
        .output()
        .expect("应能启动 lyyime-cli");
    assert!(out.status.success());
    assert!(
        out.stderr.is_empty(),
        "正常回放不应有警告:{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 6, "5 个字母 + 1 个空格,每键一行:{:?}", stdout);
    assert!(lines[0].contains("\"key\":\"n\""), "{:?}", lines[0]);
    assert!(
        lines[0].contains("\"t\":\"preedit\",\"s\":\"n\""),
        "{:?}",
        lines[0]
    );
    assert!(lines[0].contains("\"t\":\"cands\""), "{:?}", lines[0]);
    // 最后一行:空格顶屏"你好"。
    assert!(
        lines[5].contains("\"t\":\"commit\",\"s\":\"你好\""),
        "末行应含 commit 你好:{:?}",
        lines[5]
    );
    // 退出前 flush:学习词应写入 HOME 临时目录下的 user.tsv。
    assert!(
        td.path.join(".local/share/lyyime/user.tsv").exists(),
        "CLI 退出时应把学习词落盘到 $HOME/.local/share/lyyime/user.tsv"
    );
}

#[test]
fn cli_cands_输出候选明细() {
    let td = TempDir::new();
    let out = cli(&td)
        .args([
            "--data-dir",
            fixtures().to_str().unwrap(),
            "--cands",
            "replay",
            "ni",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    let last = stdout.lines().last().unwrap();
    assert!(last.contains("\"text\":\"你\""), "{:?}", last);
    assert!(last.contains("\"comment\":\"wqiy\""), "{:?}", last);
    assert!(last.contains("\"kind\":\"Pinyin\""), "{:?}", last);
    assert!(last.contains("\"i\":1"), "{:?}", last);
}

#[test]
fn cli_shift标点翻页回放() {
    // <shift> 切英文 → 字母直通;<shift> 切回;"ni," 上屏"你"+中文逗号。
    let td = TempDir::new();
    let out = cli(&td)
        .args([
            "--data-dir",
            fixtures().to_str().unwrap(),
            "replay",
            "<shift>n<shift>ni,",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout
        .lines()
        .next()
        .unwrap()
        .contains("\"t\":\"mode\",\"m\":1"));
    assert!(stdout.contains("\"key\":\"n\"") && stdout.contains("\"t\":\"pass\""));
    let last = stdout.lines().last().unwrap();
    assert!(last.contains("\"t\":\"commit\",\"s\":\"你\""), "{:?}", last);
    assert!(last.contains("\"t\":\"commit\",\"s\":\",\""), "{:?}", last);
}

#[test]
fn cli_参数错误_退出码2与人话提示() {
    let td = TempDir::new();
    // 无参数。
    let out = cli(&td).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("用法") || stderr.contains("错误"),
        "{:?}",
        stderr
    );
    // 未知子命令。
    let out = cli(&td)
        .args(["--data-dir", fixtures().to_str().unwrap(), "dance", "abc"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    // 未知键标记。
    let out = cli(&td)
        .args([
            "--data-dir",
            fixtures().to_str().unwrap(),
            "replay",
            "<boom>",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("<boom>"), "{:?}", stderr);
}
