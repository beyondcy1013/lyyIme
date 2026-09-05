//! 内置输入法目录(id → dnf 包名/引擎名映射)。
//! 包名以 openEuler 24.03 为准(Fedora 系个别子包名不同,见 note)。

use crate::model::ImeKind;

pub struct CatalogEntry {
    /// 目录 id(ime-add / ime-remove 使用)
    pub id: &'static str,
    /// 显示名
    pub name: &'static str,
    pub kind: ImeKind,
    /// dnf 包名(安装顺序即数组顺序)
    pub packages: &'static [&'static str],
    /// 安装后提供的 ibus 引擎 id(用于写入 preload-engines)
    pub engines: &'static [&'static str],
    /// 是否中文输入法(remove 保护规则用)
    pub chinese: bool,
    pub note: &'static str,
}

const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        id: "pinyin",
        name: "智能拼音(ibus-libpinyin)",
        kind: ImeKind::IbusEngine,
        packages: &["ibus-libpinyin"],
        engines: &["libpinyin", "libbopomofo"],
        chinese: true,
        note: "全拼/双拼,openEuler 官方仓库",
    },
    CatalogEntry {
        id: "wubi-haifeng",
        name: "海峰五笔 86(ibus-table)",
        kind: ImeKind::IbusEngine,
        packages: &["ibus-table-chinese-wubi-haifeng"],
        engines: &["table:wubi-haifeng86", "table:wubi-haifeng"],
        chinese: true,
        note: "openEuler 包名无 86 后缀;依赖 ibus-table/ibus-table-chinese 由 dnf 自动带入",
    },
    CatalogEntry {
        id: "wubi-jidian",
        name: "极点五笔 86(ibus-table)",
        kind: ImeKind::IbusEngine,
        packages: &["ibus-table-chinese-wubi-jidian"],
        engines: &["table:wubi-jidian86"],
        chinese: true,
        note: "openEuler 包名无 86 后缀",
    },
    CatalogEntry {
        id: "rime",
        name: "中州韵(ibus-rime)",
        kind: ImeKind::IbusEngine,
        packages: &["ibus-rime"],
        engines: &["rime"],
        chinese: true,
        note: "高度可定制;首次部署需触发 deployment",
    },
    CatalogEntry {
        id: "ibus-table",
        name: "ibus 码表框架(被五笔表依赖)",
        kind: ImeKind::Other,
        packages: &["ibus-table", "ibus-table-chinese"],
        engines: &[],
        chinese: false,
        note: "通常无需单独安装,装五笔表时会自动带入",
    },
    CatalogEntry {
        id: "fcitx5",
        name: "Fcitx 5 框架(与 ibus 二选一)",
        kind: ImeKind::Fcitx5,
        packages: &[
            "fcitx5",
            "fcitx5-chinese-addons",
            "fcitx5-table",
            "fcitx5-gtk",
            "fcitx5-qt",
            "fcitx5-configtool",
        ],
        engines: &[],
        chinese: true,
        note: "独立框架;安装后需改 GTK_IM_MODULE/QT_IM_MODULE/XMODIFIERS 并注销重登",
    },
];

pub fn catalog() -> &'static [CatalogEntry] {
    CATALOG
}

/// 按 目录 id 或 引擎 id 查找(如 "pinyin"、"table:wubi-haifeng86"、"rime")
pub fn find(id: &str) -> Option<&'static CatalogEntry> {
    CATALOG
        .iter()
        .find(|c| c.id == id || c.engines.contains(&id))
}

/// 引擎 id → 所属目录项
pub fn find_by_engine(engine: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|c| c.engines.contains(&engine))
}

/// 引擎 id → 展示用包名(取目录项第一个包;无映射返回 None)
pub fn package_for_engine(engine: &str) -> Option<&'static str> {
    find_by_engine(engine).and_then(|c| c.packages.first().copied())
}

pub fn ids() -> Vec<&'static str> {
    CATALOG.iter().map(|c| c.id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_by_id_and_engine() {
        assert_eq!(find("pinyin").unwrap().packages, &["ibus-libpinyin"]);
        assert_eq!(find("table:wubi-haifeng86").unwrap().id, "wubi-haifeng");
        assert_eq!(find("table:wubi-jidian86").unwrap().id, "wubi-jidian");
        assert!(find("does-not-exist").is_none());
    }

    #[test]
    fn package_mapping_for_engines() {
        assert_eq!(package_for_engine("libpinyin"), Some("ibus-libpinyin"));
        assert_eq!(
            package_for_engine("table:wubi-haifeng86"),
            Some("ibus-table-chinese-wubi-haifeng")
        );
        assert_eq!(package_for_engine("xkb:us::eng"), None);
    }

    #[test]
    fn wubi_entries_are_chinese_and_fcitx5_lists_packages() {
        assert!(find("wubi-haifeng").unwrap().chinese);
        let f = find("fcitx5").unwrap();
        assert_eq!(f.kind, ImeKind::Fcitx5);
        assert!(f.packages.contains(&"fcitx5-chinese-addons"));
        assert!(ids().contains(&"rime"));
    }
}
