//! Built-in category catalog for parse/resolve (mirrors `categories` rows).

use super::fold::fold;
use super::locale::Locale;

#[derive(Debug, Clone, Copy)]
pub struct CategoryDef {
    pub key: &'static str,
    pub name_vi: &'static str,
    pub name_en: &'static str,
}

pub const CATEGORIES: &[CategoryDef] = &[
    CategoryDef {
        key: "an-uong",
        name_vi: "Ăn uống",
        name_en: "Food & drink",
    },
    CategoryDef {
        key: "thuc-pham",
        name_vi: "Thực phẩm",
        name_en: "Groceries",
    },
    CategoryDef {
        key: "di-lai",
        name_vi: "Đi lại",
        name_en: "Transport",
    },
    CategoryDef {
        key: "hoa-don",
        name_vi: "Hóa đơn",
        name_en: "Bills",
    },
    CategoryDef {
        key: "mua-sam",
        name_vi: "Mua sắm",
        name_en: "Shopping",
    },
    CategoryDef {
        key: "suc-khoe",
        name_vi: "Sức khỏe",
        name_en: "Health",
    },
    CategoryDef {
        key: "giai-tri",
        name_vi: "Giải trí",
        name_en: "Entertainment",
    },
    CategoryDef {
        key: "giao-duc",
        name_vi: "Giáo dục",
        name_en: "Education",
    },
    CategoryDef {
        key: "nha-o",
        name_vi: "Nhà ở",
        name_en: "Housing",
    },
    CategoryDef {
        key: "thu-nhap",
        name_vi: "Thu nhập",
        name_en: "Income",
    },
    CategoryDef {
        key: "hoan-tien",
        name_vi: "Hoàn tiền",
        name_en: "Refund",
    },
    CategoryDef {
        key: "chuyen-khoan",
        name_vi: "Chuyển khoản",
        name_en: "Transfer",
    },
    CategoryDef {
        key: "khac",
        name_vi: "Khác",
        name_en: "Other",
    },
];

pub fn category_display(locale: Locale, key: &str) -> String {
    CATEGORIES
        .iter()
        .find(|c| c.key == key)
        .map(|c| match locale {
            Locale::Vi => c.name_vi.to_string(),
            Locale::En => c.name_en.to_string(),
        })
        .unwrap_or_else(|| key.to_string())
}

/// Resolve user input to a category key (key, VN name, or EN name).
pub fn resolve_category_key(input: &str) -> Option<&'static str> {
    let raw = input.trim();
    if raw.is_empty() {
        return None;
    }
    let folded = fold(raw);
    for cat in CATEGORIES {
        if folded == fold(cat.key)
            || folded == fold(cat.name_vi)
            || folded == fold(cat.name_en)
            || folded == fold(&cat.name_en.replace('&', "and"))
        {
            return Some(cat.key);
        }
    }
    // Tolerate "food drink" vs "food & drink"
    for cat in CATEGORIES {
        let en_plain = fold(&cat.name_en.replace('&', " ").replace("  ", " "));
        if folded == en_plain {
            return Some(cat.key);
        }
    }
    // First-word aliases: "food" → Food & drink, "an" alone is too short / ambiguous.
    for cat in CATEGORIES {
        let en_folded = fold(cat.name_en);
        let vi_folded = fold(cat.name_vi);
        let en_first = en_folded.split_whitespace().next().unwrap_or("");
        let vi_first = vi_folded.split_whitespace().next().unwrap_or("");
        if folded.len() >= 3 && (folded == en_first || folded == vi_first) {
            return Some(cat.key);
        }
    }
    None
}

pub fn format_category_list(locale: Locale) -> String {
    let mut lines = Vec::with_capacity(CATEGORIES.len());
    for cat in CATEGORIES {
        let name = match locale {
            Locale::Vi => cat.name_vi,
            Locale::En => cat.name_en,
        };
        lines.push(format!("• {key} — {name}", key = cat.key));
    }
    lines.join("\n")
}
