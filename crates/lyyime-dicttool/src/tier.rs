//! 生成单字分档表 char_tier.tsv(GB2312 一级/二级常用字档)。
//!
//! 背景:海峰码表 freq 对大量字是默认值,拼音语料对生僻字也是填充值——
//! 两者都无法回答"这是不是常用字"。GB2312 一级(3755)/二级(3008)是
//! 五笔86 原版码表隐含的常用字边界,程序化枚举内嵌于此,零外部数据。
//! core/float 引擎加载该表做生僻字沉底(见 ARCHITECTURE §5)。

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// GB2312 分档表(构建期嵌入,`char\ttier`,# 注释)。
const GB2312_TIER: &str = include_str!("../gb2312_tier.tsv");

pub fn run(out_dir: &Path) -> Result<()> {
    fs::create_dir_all(out_dir)
        .with_context(|| format!("创建目录失败: {}", out_dir.display()))?;
    let dst = out_dir.join("char_tier.tsv");
    fs::write(&dst, GB2312_TIER)
        .with_context(|| format!("写入失败: {}", dst.display()))?;
    let rows = GB2312_TIER
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .count();
    println!("char_tier.tsv: {rows} 字 → {}", dst.display());
    Ok(())
}
