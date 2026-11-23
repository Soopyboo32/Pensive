//! The user's settings file, as its text and as checked values, and every declared setting.

use std::collections::{BTreeSet, HashMap};

use gpui::{App, Global, UpdateGlobal};
use toml_edit::{DocumentMut, Item, TableLike};

use crate::{AnySetting, Page};

#[derive(Default)]
pub struct Settings {
    /// Each setting with the page it's listed on.
    declared: Vec<(Page, &'static dyn AnySetting)>,
    /// Keys that a second setting tried to declare.
    clashes: BTreeSet<&'static str>,
    source: String,
    /// The values in `source` that are valid for a declared setting, by its dotted key.
    values: HashMap<&'static str, toml::Value>,
}

impl Global for Settings {}

/// How the settings and theme files report a value they can't use. `key` is its dotted path.
pub fn bad_value(key: &str, reason: impl std::fmt::Display) -> String {
    format!("`{key}` is ignored because its value is wrong: {reason}")
}

impl Settings {
    /// Adds `settings` to the ones the palette and the settings view list, on `page`. Call it
    /// before loading the file, so their keys are known. Declaring the same setting again, as a
    /// mock does in a running app, lists it once. When two settings share a key the first keeps
    /// it, and `load` reports the clash.
    pub fn declare(
        page: Page,
        settings: impl IntoIterator<Item = &'static dyn AnySetting>,
        cx: &mut App,
    ) {
        let store = cx.default_global::<Settings>();
        for setting in settings {
            match store.find(setting.key()) {
                None => store.declared.push((page, setting)),
                Some(known) if !std::ptr::addr_eq(known, setting) => {
                    store.clashes.insert(setting.key());
                }
                Some(_) => {}
            }
        }
    }

    /// Every declared setting with its page, in the order they were declared.
    pub fn declared(&self) -> impl Iterator<Item = (Page, &'static dyn AnySetting)> {
        self.declared.iter().copied()
    }

    pub fn find(&self, key: &str) -> Option<&'static dyn AnySetting> {
        self.declared()
            .map(|(_, setting)| setting)
            .find(|setting| setting.key() == key)
    }

    /// The file's text, with any changes made in the app.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Reads `source` as the settings file and returns what's wrong with it. When it isn't valid
    /// TOML, the values stay as they were.
    pub fn load(source: &str, cx: &mut App) -> Vec<String> {
        cx.default_global::<Settings>();
        Settings::update_global(cx, |settings, cx| settings.read_source(source, cx))
    }

    fn read_source(&mut self, source: &str, cx: &App) -> Vec<String> {
        self.source = source.to_string();
        let mut problems: Vec<String> = self
            .clashes
            .iter()
            .map(|key| format!("Two settings are declared with the key `{key}`"))
            .collect();
        match source.parse::<toml::Table>() {
            Ok(table) => {
                let mut values = HashMap::new();
                self.read_table(table, "", &mut values, &mut problems, cx);
                self.values = values;
            }
            Err(error) => problems.push(error.to_string()),
        }
        problems
    }

    pub(crate) fn value(&self, key: &str) -> Option<&toml::Value> {
        self.values.get(key)
    }

    /// Sets `key` to `value`, or removes it for `None`, in the values and in the file's text. While
    /// the text isn't valid TOML the change stays in memory, so the file is left for the user to
    /// fix.
    pub(crate) fn edit(&mut self, key: &'static str, value: Option<toml::Value>) {
        match &value {
            Some(value) => self.values.insert(key, value.clone()),
            None => self.values.remove(key),
        };
        let Ok(mut document) = self.source.parse::<DocumentMut>() else {
            return;
        };
        let root = document.as_table_mut();
        match value {
            Some(value) => {
                if let Ok(value) = value.to_string().parse() {
                    write_item(root, key, value);
                }
            }
            None => {
                remove_item(root, key);
            }
        }
        self.source = document.to_string();
    }

    /// Adds the entries of `table` that are valid settings to `values`, and reports the others in
    /// `problems`.
    fn read_table(
        &self,
        table: toml::Table,
        prefix: &str,
        values: &mut HashMap<&'static str, toml::Value>,
        problems: &mut Vec<String>,
        cx: &App,
    ) {
        for (name, value) in table {
            let path = match prefix {
                "" => name.clone(),
                prefix => format!("{prefix}.{name}"),
            };
            // A quoted key with a dot in it is one key, and no setting has one.
            let setting = self.find(&path).filter(|_| !name.contains('.'));
            match (setting, value) {
                (Some(setting), value) => match setting.check(&value, cx) {
                    Ok(()) => {
                        values.insert(setting.key(), value);
                    }
                    Err(reason) => problems.push(bad_value(&path, reason)),
                },
                (None, toml::Value::Table(table)) if !name.contains('.') => {
                    self.read_table(table, &path, values, problems, cx);
                }
                (None, _) => problems.push(format!("`{path}` isn't a setting")),
            }
        }
    }
}

/// Writes `value` at the dotted `key`, adding the tables it sits in and keeping the file's comments
/// and layout.
fn write_item(table: &mut dyn TableLike, key: &str, mut value: toml_edit::Value) {
    let Some((parent, rest)) = key.split_once('.') else {
        match table.get_mut(key).and_then(Item::as_value_mut) {
            Some(old) => {
                *value.decor_mut() = old.decor().clone();
                *old = value;
            }
            None => {
                table.insert(key, Item::Value(value));
            }
        }
        return;
    };
    if table.get(parent).and_then(Item::as_table_like).is_none() {
        table.insert(parent, toml_edit::table());
    }
    if let Some(child) = table.get_mut(parent).and_then(Item::as_table_like_mut) {
        write_item(child, rest, value);
    }
}

/// Removes the dotted `key`, and each table that leaves empty unless a comment sits on it. Returns
/// whether the key was there.
fn remove_item(table: &mut dyn TableLike, key: &str) -> bool {
    let Some((parent, rest)) = key.split_once('.') else {
        return table.remove(key).is_some();
    };
    let Some(child) = table.get_mut(parent).and_then(Item::as_table_like_mut) else {
        return false;
    };
    let removed = remove_item(child, rest);
    if removed && child.is_empty() && !has_comment(table, parent) {
        table.remove(parent);
    }
    removed
}

/// Whether a comment sits on the header or key of the entry `name`.
fn has_comment(table: &dyn TableLike, name: &str) -> bool {
    let item_decor = table.get(name).and_then(|item| match item {
        Item::Table(table) => Some(table.decor()),
        Item::Value(value) => Some(value.decor()),
        _ => None,
    });
    let key_decor = table.key(name).map(|key| key.leaf_decor());
    [item_decor, key_decor]
        .into_iter()
        .flatten()
        .flat_map(|decor| [decor.prefix(), decor.suffix()])
        .flatten()
        .any(|text| text.as_str().is_some_and(|text| text.contains('#')))
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;
    use crate::{Category, Choice, Control, Setting};

    static PALETTE_CATEGORY: Category = Category::new("Command palette", "Tabs and Active");
    static APPEARANCE: Category = Category::new("Appearance", "Size");
    const PALETTE: Page = Page::new(&PALETTE_CATEGORY, "Tabs");

    static SHOW_TABS: Setting<bool> = Setting {
        key: "palette.show_tabs",
        name: "Tabs",
        description: "",
        default: true,
        control: Control::Toggle,
    };

    static DAYS: Setting<u32> = Setting {
        key: "palette.active_days",
        name: "Active",
        description: "",
        default: 7,
        control: Control::Select(|_| vec![Choice::new(7, "7 days")]),
    };

    static SCRIM: Setting<u32> = Setting {
        key: "palette.scrim",
        name: "Scrim",
        description: "",
        default: 50,
        control: Control::Segmented(|_| vec![Choice::new(0, "None"), Choice::new(50, "50%")]),
    };

    static OPACITY: Setting<u32> = Setting {
        key: "palette.opacity",
        name: "Opacity",
        description: "",
        default: 100,
        control: Control::Stepper(|_| vec![Choice::new(70, "70%"), Choice::new(100, "100%")]),
    };

    static DENSITY: Setting<String> = Setting {
        key: "density",
        name: "Density",
        description: "",
        default: String::new(),
        control: Control::Select(|_| Vec::new()),
    };

    fn settings(source: &str, cx: &App) -> (Settings, Vec<String>) {
        let mut settings = Settings {
            declared: vec![
                (PALETTE, &SHOW_TABS),
                (PALETTE, &DAYS),
                (PALETTE, &SCRIM),
                (PALETTE, &OPACITY),
                (Page::new(&APPEARANCE, "Size"), &DENSITY),
            ],
            ..Default::default()
        };
        let problems = settings.read_source(source, cx);
        (settings, problems)
    }

    /// The file's text after `edits`, each a key and its new value or `None` to remove it.
    fn edited(source: &str, edits: &[(&'static str, Option<toml::Value>)], cx: &App) -> String {
        let (mut settings, _) = settings(source, cx);
        for (key, value) in edits {
            settings.edit(key, value.clone());
        }
        settings.source().to_string()
    }

    #[gpui::test]
    fn unknown_keys_and_bad_values_are_reported_and_the_rest_apply(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let (settings, problems) = settings(
                r#"
                density = "dense"

                [palette]
                show_tab = false
                active_days = "a week"
                "#,
                cx,
            );
            assert_eq!(settings.value("density").unwrap().as_str(), Some("dense"));
            assert!(settings.value("palette.active_days").is_none());
            assert_eq!(problems.len(), 2);
            assert!(problems[0].starts_with("`palette.show_tab` isn't a setting"));
            assert!(problems[1].starts_with("`palette.active_days` is ignored because"));
        });
    }

    #[gpui::test]
    fn values_outside_the_choices_are_reported_like_bad_values(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let (settings, problems) = settings("[palette]\nscrim = 999\nopacity = 0\n", cx);
            assert!(settings.value("palette.scrim").is_none());
            assert!(settings.value("palette.opacity").is_none());
            assert_eq!(
                problems,
                [
                    bad_value("palette.scrim", "expected one of 0, 50"),
                    bad_value("palette.opacity", "expected one of 70, 100"),
                ]
            );
        });
    }

    #[gpui::test]
    fn a_select_accepts_values_it_doesnt_list(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let (settings, problems) = settings("[palette]\nactive_days = 12\n", cx);
            assert!(problems.is_empty(), "{problems:?}");
            let days = settings.value("palette.active_days").unwrap();
            assert_eq!(days.as_integer(), Some(12));
        });
    }

    #[gpui::test]
    fn a_quoted_key_with_a_dot_isnt_the_setting(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let (settings, problems) = settings("\"palette.show_tabs\" = false\n", cx);
            assert!(settings.value("palette.show_tabs").is_none());
            assert_eq!(problems, ["`palette.show_tabs` isn't a setting"]);
        });
    }

    static FIRST: Setting<bool> = Setting {
        key: "shared",
        name: "First",
        description: "",
        default: true,
        control: Control::Toggle,
    };

    static SECOND: Setting<bool> = Setting {
        key: "shared",
        name: "Second",
        description: "",
        default: true,
        control: Control::Toggle,
    };

    #[gpui::test]
    fn the_first_of_two_settings_with_one_key_wins_and_the_clash_is_reported(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            let settings: [&'static dyn AnySetting; 3] = [&FIRST, &FIRST, &SECOND];
            Settings::declare(PALETTE, settings, cx);
            Settings::declare(PALETTE, [&SECOND as &dyn AnySetting], cx);
            let store = cx.global::<Settings>();
            assert_eq!(store.find("shared").unwrap().name(), "First");
            assert_eq!(store.declared().count(), 1);
            assert_eq!(
                Settings::load("", cx),
                ["Two settings are declared with the key `shared`"]
            );
        });
    }

    #[gpui::test]
    fn invalid_toml_keeps_the_last_good_values(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let (mut settings, _) = settings("density = \"dense\"", cx);
            let problems = settings.read_source("density = ", cx);
            assert_eq!(problems.len(), 1);
            assert_eq!(settings.value("density").unwrap().as_str(), Some("dense"));
        });
    }

    #[gpui::test]
    fn edits_keep_comments_and_add_tables(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let (mut settings, _) = settings("# Mine\ndensity = \"dense\" # for the laptop\n", cx);
            settings.edit("density", Some("calm".into()));
            settings.edit("palette.show_tabs", Some(false.into()));
            assert_eq!(
                settings.source(),
                "# Mine\ndensity = \"calm\" # for the laptop\n\n[palette]\nshow_tabs = false\n"
            );
            settings.edit("palette.show_tabs", None);
            assert!(settings.value("palette.show_tabs").is_none());
            assert_eq!(
                settings.source(),
                "# Mine\ndensity = \"calm\" # for the laptop\n"
            );
        });
    }

    #[gpui::test]
    fn removing_a_tables_last_key_removes_the_table(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let remove = [("palette.show_tabs", None)];
            for source in [
                "[palette]\nshow_tabs = false\n",
                "palette = { show_tabs = false }\n",
                "palette.show_tabs = false\n",
            ] {
                assert_eq!(edited(source, &remove, cx), "", "{source}");
            }
            assert_eq!(
                edited("[palette]\nshow_tabs = false\nscrim = 0\n", &remove, cx),
                "[palette]\nscrim = 0\n"
            );
        });
    }

    #[gpui::test]
    fn a_table_with_a_comment_stays_when_its_last_key_goes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let source = "# The palette\n[palette]\nshow_tabs = false\n";
            assert_eq!(
                edited(source, &[("palette.show_tabs", None)], cx),
                "# The palette\n[palette]\n"
            );
        });
    }

    #[gpui::test]
    fn removing_a_key_that_isnt_there_changes_nothing(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let remove = [("palette.show_tabs", None)];
            assert_eq!(edited("", &remove, cx), "");
            assert_eq!(edited("[palette]\n", &remove, cx), "[palette]\n");
        });
    }

    #[gpui::test]
    fn edits_replace_whatever_the_file_has_in_a_tables_place(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let write = [("palette.show_tabs", Some(false.into()))];
            for source in [
                "palette = 3\n",
                "[[palette]]\nscrim = 0\n",
                "palette = { scrim = 0 }\n",
            ] {
                let text = edited(source, &write, cx);
                let (settings, _) = settings(&text, cx);
                let value = settings.value("palette.show_tabs");
                assert_eq!(value.and_then(toml::Value::as_bool), Some(false), "{text}");
            }
        });
    }

    #[gpui::test]
    fn a_table_value_is_written_so_it_reads_back(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let value: toml::Table = "size = 2\nnames = [\"a\"]".parse().unwrap();
            let value = toml::Value::Table(value);
            let text = edited("", &[("density", Some(value.clone()))], cx);
            assert_eq!(text.parse::<toml::Table>().unwrap()["density"], value);
        });
    }

    #[gpui::test]
    fn edits_to_a_broken_file_stay_in_memory(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let (mut settings, _) = settings("density = ", cx);
            settings.edit("density", Some("dense".into()));
            assert_eq!(settings.source(), "density = ");
            assert_eq!(settings.value("density").unwrap().as_str(), Some("dense"));
        });
    }
}
