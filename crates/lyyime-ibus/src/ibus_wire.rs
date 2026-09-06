//! IBus 对象的 GVariant 报文构建。
//!
//! 签名逐一对照本机 ibus 1.5.29 真机抓包(2026-09-06,dbus-monitor 于
//! ibus 私有总线;python 引擎实测)。新版 ibus 序列化 = 类型名字符串 +
//! a{sv} 属性表 + 自身字段:
//!   IBusText       (s a{sv} s v)          类型名/属性/文本/属性表(v=IBusAttrList)
//!   IBusAttrList   (s a{sv} av)           属性表(每项 v=IBusAttribute)
//!   IBusAttribute  (s a{sv} u u u u)      attr_type/value/start/end
//!   IBusLookupTable(s a{sv} u u b b i av av) 页大小/游标/可见/循环/朝向/候选/标签
//!   IBusProperty   (s a{sv} s u v s v b b u v v) key/类型/label/icon/tooltip/
//!                                                 sensitive/visible/state/子属性/symbol
//!   IBusPropList   (s a{sv} av)
//!   IBusComponent  (s a{sv} s*8 av av)    8 个字符串 + 观察配置 + 引擎列表
//!   IBusEngineDesc (s a{sv} s*8 u s*8)
//! 下划线属性用 IBusAttrUnderline.SINGLE(attr_type=1, value=1)。

use zbus::zvariant::{Dict, OwnedValue, Signature, StructureBuilder, Value};

pub const PROP_TYPE_NORMAL: u32 = 0;
pub const PROP_TYPE_TOGGLE: u32 = 1;
pub const PROP_TYPE_MENU: u32 = 2;
pub const PROP_STATE_UNCHECKED: u32 = 0;
/// IBus.Orientation.HORIZONTAL
pub const ORIENTATION_HORIZONTAL: i32 = 0;
pub const PREEDIT_FOCUS_MODE_CLEAR: u32 = 0;
pub const PREEDIT_FOCUS_MODE_COMMIT: u32 = 1;

pub const VERSION: &str = "0.2.0";

fn to_owned(v: Value<'static>) -> OwnedValue {
    OwnedValue::try_from(v).expect("OwnedValue 转换失败")
}

fn empty_dict() -> Value<'static> {
    // 空的 a{sv}(IBusSerializable 属性表;python 引擎也恒为空)
    let d = Dict::new(
        Signature::from_string_unchecked("s".into()),
        Signature::from_string_unchecked("v".into()),
    );
    Value::Dict(d)
}

/// (s a{sv} av) 装箱为 variant(IBusText 的第 4 字段是 v)
fn attr_list_value(underline: Option<usize>) -> Value<'static> {
    let mut list: Vec<Value> = Vec::new();
    if let Some(n) = underline {
        let attr = StructureBuilder::new()
            .add_field("IBusAttribute".to_string())
            .append_field(empty_dict())
            .add_field(1u32) // ATTR_TYPE_UNDERLINE
            .add_field(1u32) // UNDERLINE_SINGLE
            .add_field(0u32)
            .add_field(n as u32)
            .build();
        list.push(Value::from(attr));
    }
    Value::Value(Box::new(
        StructureBuilder::new()
            .add_field("IBusAttrList".to_string())
            .append_field(empty_dict())
            .add_field(list)
            .build()
            .into(),
    ))
}

/// (s a{sv} s v)
fn text_value(text: &str, underline: bool) -> Value<'static> {
    let underline = if underline { Some(text.chars().count()) } else { None };
    StructureBuilder::new()
        .add_field("IBusText".to_string())
        .append_field(empty_dict())
        .add_field(text.to_string())
        .append_field(attr_list_value(underline))
        .build()
        .into()
}

/// 构造 IBusText;show=true 时带单下划线属性(预编辑用)。
pub fn ibus_text(text: &str, underline: bool) -> OwnedValue {
    to_owned(text_value(text, underline))
}

/// (s a{sv} u u b b i av av)
/// 候选显示为 "文本 注释"(python 版同款拼接),标签为序号 1–9。
pub fn lookup_table(cands: &[(String, String)]) -> OwnedValue {
    let mut candidates: Vec<Value> = Vec::new();
    let mut labels: Vec<Value> = Vec::new();
    for (i, (text, comment)) in cands.iter().enumerate() {
        let label_text = if comment.is_empty() {
            text.clone()
        } else {
            format!("{text} {comment}")
        };
        candidates.push(text_value(&label_text, false));
        if i < 9 {
            labels.push(text_value(&(i + 1).to_string(), false));
        }
    }
    let v = StructureBuilder::new()
        .add_field("IBusLookupTable".to_string())
        .append_field(empty_dict())
        .add_field(cands.len().max(1) as u32)
        .add_field(0u32)
        .add_field(true)
        .add_field(false) // 翻页边界由 core 钳制,不循环
        .add_field(ORIENTATION_HORIZONTAL)
        .append_field(Value::from(candidates))
        .append_field(Value::from(labels))
        .build()
        .into();
    to_owned(v)
}

/// (s a{sv} av)
fn proplist_value(items: Vec<OwnedValue>) -> Value<'static> {
    let items: Vec<Value> = items.into_iter().map(Value::from).collect();
    Value::Value(Box::new(
        StructureBuilder::new()
            .add_field("IBusPropList".to_string())
            .append_field(empty_dict())
            .append_field(Value::from(items))
            .build()
            .into(),
    ))
}

/// (s a{sv} s u v s v b b u v v)
#[allow(clippy::too_many_arguments)]
pub fn property(
    key: &str,
    prop_type: u32,
    label: &str,
    icon: &str,
    tooltip: &str,
    symbol: &str,
    sub_props: Vec<OwnedValue>,
) -> Value<'static> {
    let v: Value<'static> = StructureBuilder::new()
        .add_field("IBusProperty".to_string())
        .append_field(empty_dict())
        .add_field(key.to_string())
        .add_field(prop_type)
        .append_field(Value::Value(Box::new(text_value(label, false))))
        .add_field(icon.to_string())
        .append_field(Value::Value(Box::new(text_value(tooltip, false))))
        .add_field(true)
        .add_field(true)
        .add_field(PROP_STATE_UNCHECKED)
        .append_field(proplist_value(sub_props))
        .append_field(Value::Value(Box::new(text_value(symbol, false))))
        .build()
        .into();
    v
}

/// (s a{sv} s*8 u s*8) —— 对照真机抓包字段顺序:
/// name/longname/description/language/license/author/icon/layout + rank +
/// layout_variant/symbol/textdomain/icon_prop_key/setup 及 3 个保留空位。
fn engine_desc_value(
    name: &str,
    longname: &str,
    description: &str,
    language: &str,
    icon: &str,
    symbol: &str,
) -> Value<'static> {
    StructureBuilder::new()
        .add_field("IBusEngineDesc".to_string())
        .append_field(empty_dict())
        .add_field(name.to_string())
        .add_field(longname.to_string())
        .add_field(description.to_string())
        .add_field(language.to_string())
        .add_field("GPLv3".to_string())
        .add_field("lyyIme contributors".to_string())
        .add_field(icon.to_string())
        .add_field("default".to_string())
        .add_field(70u32)
        .add_field("") // layout_variant
        .add_field(symbol.to_string())
        .add_field("") // textdomain
        .add_field("") // icon_prop_key
        .add_field("") // setup
        .add_field("") // 其余保留字段(ibus 1.5.29 序列化尾部)
        .add_field("")
        .add_field("")
        .build()
        .into()
}

/// (s a{sv} s*8 av av)
fn component_value(exec: &str, engine: OwnedValue) -> Value<'static> {
    let engines: Vec<Value> = vec![Value::from(engine)];
    StructureBuilder::new()
        .add_field("IBusComponent".to_string())
        .append_field(empty_dict())
        .add_field("org.freedesktop.IBus.Lyyime".to_string())
        .add_field("lyyIme Component".to_string())
        .add_field(VERSION.to_string())
        .add_field("GPLv3".to_string())
        .add_field("lyyIme contributors".to_string())
        .add_field("") // homepage
        .add_field(exec.to_string())
        .add_field("lyyime".to_string()) // textdomain
        .append_field(Value::from(Vec::<Value>::new())) // observed conf
        .append_field(Value::from(engines))
        .build()
        .into()
}

pub fn component(exec: &str, engine: OwnedValue) -> OwnedValue {
    to_owned(component_value(exec, engine))
}

pub fn engine_desc(
    name: &str,
    longname: &str,
    description: &str,
    language: &str,
    icon: &str,
    symbol: &str,
) -> OwnedValue {
    to_owned(engine_desc_value(name, longname, description, language, icon, symbol))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::DynamicType;

    #[test]
    fn ibus_text_signature() {
        let v = ibus_text("你好", true);
        // OwnedValue 的 DynamicType 是 "v",经解引用取内层签名
        let inner: &Value = &v;
        assert_eq!(inner.value_signature(), "(sa{sv}sv)");
    }

    #[test]
    fn lookup_table_signature() {
        let v = lookup_table(&[("你好".to_string(), "wqvb".to_string())]);
        let inner: &Value = &v;
        assert_eq!(inner.value_signature(), "(sa{sv}uubbiavav)");
    }

    #[test]
    fn property_and_proplist_signature() {
        let p = property("InputMode", PROP_TYPE_TOGGLE, "中英切换", "icon", "tip", "中", vec![]);
        let inner: &Value = &p;
        // property 返回 Structure 形式的 Value;sub_props/symbol 均为 v 装箱
        assert_eq!(
            inner.value_signature(),
            "(sa{sv}su(sa{sv}sv)s(sa{sv}sv)bbuvv)"
        );
        // proplist_value 本身返回的是 v(装箱后), 内层才是 (sa{sv}av)
        let pl = proplist_value(vec![OwnedValue::try_from(p).expect("ok")]);
        if let Value::Value(inner) = &pl {
            assert_eq!(inner.value_signature().as_str(), "(sa{sv}av)");
        } else {
            panic!("proplist 应为 variant 装箱");
        }
    }

    #[test]
    fn gv_bytes_for_python_crosscheck() {
        let p = property(
            "InputMode",
            PROP_TYPE_TOGGLE,
            "中英切换(Shift 单击)",
            "/home/common/icons/lyyime-zh.svg",
            "当前:中文。单击切换中/英文,与 Shift 单击同效。",
            "中",
            Vec::new(),
        );
        let boxed = Value::Value(Box::new(p));
        let d = zvariant::to_bytes(
            zbus::zvariant::serialized::Context::new(
                zbus::zvariant::serialized::Format::GVariant,
                zbus::zvariant::Endian::Little,
                0,
            ),
            &(boxed,),
        )
        .expect("gv to_bytes");
        println!(
            "GVHEX={}",
            d.bytes().iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
    }

    #[test]
    fn component_signature() {
        let c = component(
            "/usr/bin/ibus-engine-lyyime --ibus",
            engine_desc("lyyime", "l", "d", "zh_CN", "i", "伍"),
        );
        let inner: &Value = &c;
        assert_eq!(inner.value_signature(), "(sa{sv}ssssssssavav)");
    }
}
