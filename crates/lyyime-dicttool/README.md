# lyyime-dicttool — 词典管线

`dicttool` CLI:本机 ibus-table 码表(sqlite)→ runtime TSV;下载拼音词组/英文词频并转换。
产物格式合同见 `docs/ARCHITECTURE.md` §4。本文件记录实现关键决策。

## 子命令

```
dicttool convert [--wubi-db <path>] [--out <dir>]   # 默认海峰86 → wubi/pinyin_char/suggestion/goucima + meta.json
dicttool fetch   [--out <dir>] [--cache <dir>]      # 下载(缓存 dicts/raw)→ pinyin_phrase/english,更新 meta.json
dicttool verify  [<dir>]                            # 行数下限/列数/UTF-8/排序/抽样断言,40 项检查
dicttool query   wubi|pinyin|en <query> [--out <dir>]  # 自包含 TSV 前缀扫描,打印前 9 候选(不依赖 lyyime-core)
```

## 数据源与理由

| 文件 | 来源 | 许可 | 理由 |
|---|---|---|---|
| wubi/suggestion/goucima/pinyin_char | `/usr/share/ibus-table/tables/wubi-haifeng86.db`(serial 20101124) | 码表自带(ibus-table 海峰86,0BSD 声明于 ime 表) | 本机一手数据,含 14.8 万词组/单字 |
| pinyin_phrase.tsv 词频 | [fxsjy/jieba dict.txt](https://raw.githubusercontent.com/fxsjy/jieba/master/jieba/dict.txt) | MIT | 34.9 万条词+词频,最全的免费中文词频表 |
| pinyin_phrase.tsv 拼音标注 | [mozillazg/phrase-pinyin-data large_pinyin.txt](https://raw.githubusercontent.com/mozillazg/phrase-pinyin-data/master/large_pinyin.txt) | MIT | 41.2 万条“词:带调拼音”显式标注,避免逐字反推多音字 |
| english.tsv | [first20hours/google-10000-english 20k.txt](https://raw.githubusercontent.com/first20hours/google-10000-english/master/20k.txt) | MIT | 2 万英文词按使用频率排序,满足 ≥1 万验收 |

镜像:每个源均配置 `cdn.jsdelivr.net` 镜像,主源失败自动降级;全部失败以非 0 退出并打印手动下载建议(绝不静默造假)。缓存于 `dicts/raw/`(已 gitignore),重跑跳过已缓存文件。

## 关键决策(实测结论)

1. **tabkeys 无多码分隔符**:海峰86 phrases 表 1228 行全大写 tabkeys(如 `AAWI`)是“直接上屏”记法,
   统一 `lower()` 归一;未发现 `,`/`|` 等分隔符,故无需拆多行。
2. **声调标记实测**:`!`=阴平 `@`=阳平 `#`=上声 `$`=去声 `%`=轻声(de%→de),非数字。
   轻声与其它声调一致剥掉;另防御性再剥数字后缀(`zhong1`)。`ü` 归一为 `v`(与码表 lv/nv 一致)。
3. **排序/去重下推 SQLite**:`GROUP BY ... MAX(freq)` + `ORDER BY`,流式写出,Rust 侧 O(1) 内存;
   重复 (code,word)/(py,zi)/词频同词取 MAX。输出先写 `.tmp` 再原子改名,重跑幂等(字节一致)。
4. **拼音词组构建**:jieba 词频 ∩ phrase-pinyin 标注;未命中标注的词用 pinyin_char 逐字反推,
   多音字取频次最高读音(常用多音词如 银行/行为 多数已被直接标注,反推仅兜底);任一字无读音则弃词。
5. **english 频次**:源只有名次,`freq = 总词数 - 名次 + 1` 线性转换,保持相对大小,满足 core 的 log10 归一。
6. **行数下限(M2)**:wubi≥10万(实测 148314)、english≥1万(20000)、pinyin_char≥2万(31525,
   413 个无声调音节)、pinyin_phrase≥10万(337389)、suggestion≥10万(129583)、goucima≥2万(29685)。

## 已知局限

- wubi.tsv 保留了码表中的生僻条目(如扩展 B 区字后缀 `.`,freq=100),按“保留原词”要求未清洗。
- pinyin_phrase 约 6 成词条的拼音为逐字反推,多音字语境错误不可避免(频次排序可缓解);
  core 排序公式中全拼命中权重高于简拼,影响有限。
- english 源尾部含少量生僻/怪词(20k 列表固有),rank 靠后、频次低,不影响前 2000 高频直通判断。
- fetch 依赖 curl;代理场景请先 `export HTTPS_PROXY`。
