//! lyyime-dicttool 词典管线库。
//!
//! 模块划分:
//! - [`util`]:进度输出、原子写文件、拼音声调归一化、CJK 判定、meta.json 等公共工具
//! - [`convert`]:本机 ibus-table sqlite(海峰86)→ wubi.tsv / pinyin_char.tsv / suggestion.tsv / goucima.tsv
//! - [`fetch`]:下载拼音词组与英文词频原始词典(缓存 dicts/raw)→ pinyin_phrase.tsv / english.tsv
//! - [`verify`]:对 data/runtime 做行数/格式/排序/抽样断言
//! - [`tier`]:生成 GB2312 单字分档表 char_tier.tsv(常用/次常用档,生僻字沉底依据)
//! - [`zhen`]:ECDICT(stardict.db,en→zh)反生成 zh→en 反查表 zh_en.tsv(§15 右键"反查英文")
//! - [`entrans`]:ECDICT(stardict.db,en→zh)+ 内置缩略语校对表 → en_trans.tsv(全大写输入候选的中文翻译)
//! - [`query`]:对 TSV 做独立前缀扫描的自包含查询(不依赖 lyyime-core)

pub mod convert;
pub mod entrans;
pub mod fetch;
pub mod query;
pub mod tier;
pub mod util;
pub mod verify;
pub mod zhen;
