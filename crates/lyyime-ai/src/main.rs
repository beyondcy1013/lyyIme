//! lyyime-ai CLI —— 与原 lyyime_ai.py 的命令行契约一致:
//!   lyyime-ai --prompt "..."   # 回复写 stdout,失败写 stderr 退出码 1
//!   lyyime-ai --check          # 配置体检 + 最小连通测试(不收费的冒烟请求)
//!   lyyime-ai --config <路径>  # 指定配置文件
//! lyyime-xim(Mode B)经 LYYIME_AI_HELPER 以此方式子进程调用。
use lyyime_ai::{chat, config_path, is_configured, load_config_at};
use std::io::Read;

const USAGE: &str = "\
lyyime-ai —— lyyIme AI 助手客户端(OpenAI 兼容 /chat/completions)

用法:
  lyyime-ai -p, --prompt <提示词>   提示词;省略则从 stdin 整读
  lyyime-ai --check                 配置体检 + 最小连通测试
  lyyime-ai --config <路径>         指定配置文件(默认 ~/.config/lyyime/config.toml)
  lyyime-ai --version               版本
";

fn main() {
    let mut prompt: Option<String> = None;
    let mut check = false;
    let mut config: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-p" | "--prompt" => prompt = args.next(),
            "--check" => check = true,
            "--config" => config = args.next(),
            "--version" => {
                println!("lyyime-ai {}", lyyime_ai::VERSION);
                return;
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return;
            }
            other => {
                eprintln!("未知参数:{other}\n{USAGE}");
                std::process::exit(2);
            }
        }
    }

    let path = config
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(config_path);
    let cfg = load_config_at(&path);

    if check {
        println!("配置文件: {}", path.display());
        println!("启用: {}", if cfg.enabled { "是" } else { "否" });
        println!("API 地址: {}", if cfg.api_base.is_empty() { "(未填)" } else { &cfg.api_base });
        println!(
            "密钥: {}",
            if cfg.api_key.is_empty() { "(未填,本地服务可留空)" } else { "已填" }
        );
        println!("模型: {}", if cfg.model.is_empty() { "(未填)" } else { &cfg.model });
        if !is_configured(&cfg) {
            eprintln!("结论: 未配置完整,功能未启用。");
            std::process::exit(1);
        }
        match chat("请只回复两个字符:OK", &cfg, None) {
            Ok(reply) => {
                println!("连通测试成功,模型回复: {}", reply.chars().take(100).collect::<String>());
            }
            Err(e) => {
                eprintln!("连通测试失败: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let p = match prompt {
        Some(p) => p,
        None => {
            let mut s = String::new();
            let _ = std::io::stdin().read_to_string(&mut s);
            s
        }
    };
    let p = p.trim().to_string();
    if p.is_empty() {
        eprintln!("提示词为空");
        std::process::exit(1);
    }
    match chat(&p, &cfg, None) {
        Ok(reply) => {
            print!("{reply}");
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
