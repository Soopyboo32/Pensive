//! One setting: its key in the file, what the palette and settings window show for it, and how
//! its value is read and changed.

use gpui::{App, SharedString, UpdateGlobal};
use serde::{Serialize, de::DeserializeOwned};

use crate::Settings;

/// A group of pages, such as Appearance, declared as a `static` in the crate that owns it.
#[derive(Debug, PartialEq, Eq)]
pub struct Category {
    name: &'static str,
    description: &'static str,
}

impl Category {
    /// Every category shows a description under its name, so an empty one fails to compile.
    ///
    /// ```compile_fail
    /// static PLUGINS: settings::Category = settings::Category::new("Plugins", "");
    /// ```
    pub const fn new(name: &'static str, description: &'static str) -> Self {
        assert!(
            !description.is_empty(),
            "a settings category needs a description"
        );
        Self { name, description }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub const fn description(&self) -> &'static str {
        self.description
    }
}

/// Where a setting is listed: a category, such as Appearance, and a page within it, such as Fonts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    pub category: &'static Category,
    pub name: &'static str,
}

impl Page {
    pub const fn new(category: &'static Category, name: &'static str) -> Self {
        Self { category, name }
    }
}

/// A setting, declared as a `static` in the crate that reads it.
pub struct Setting<T: 'static> {
    /// Its dotted path in `settings.toml`, like `palette.show_tabs`.
    pub key: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub default: T,
    pub control: Control<T>,
}

/// How a setting is changed, and which values the file can give it.
///
/// Every control except [`Control::Select`] only accepts its choices. `Settings::load` reads those
/// lists while other settings still read as their defaults, so the values a list accepts can't
/// depend on another setting. Its labels and order can.
pub enum Control<T: 'static> {
    /// Two values to flip between, off first. [`Control::Toggle`] is this for a `bool`.
    Switch(fn(&App) -> [Choice<T>; 2]),
    /// Two to four short choices side by side.
    Segmented(fn(&App) -> Vec<Choice<T>>),
    /// A longer list of choices, or one only known at runtime. The file can hold a value that
    /// isn't listed, such as an installed font, and the control shows it as written.
    Select(fn(&App) -> Vec<Choice<T>>),
    /// Ordered choices stepped through one at a time, stopping at either end.
    Stepper(fn(&App) -> Vec<Choice<T>>),
}

impl Control<bool> {
    /// On or off. Only a `bool` setting can use it, so this fails to compile:
    ///
    /// ```compile_fail
    /// static STEPS: settings::Setting<u32> = settings::Setting {
    ///     key: "steps",
    ///     name: "Steps",
    ///     description: "",
    ///     default: 0,
    ///     control: settings::Control::Toggle,
    /// };
    /// ```
    #[expect(
        non_upper_case_globals,
        reason = "declarations name it the way they name the other controls"
    )]
    pub const Toggle: Self = Self::Switch(|_| [Choice::new(false, "Off"), Choice::new(true, "On")]);
}

pub struct Choice<T> {
    pub value: T,
    pub label: SharedString,
    also: Option<T>,
}

impl<T> Choice<T> {
    pub fn new(value: T, label: impl Into<SharedString>) -> Self {
        Self {
            value,
            label: label.into(),
            also: None,
        }
    }

    /// Makes `value` count as this choice too. Picking the choice still sets [`Choice::value`].
    pub fn also(mut self, value: T) -> Self {
        self.also = Some(value);
        self
    }

    fn values(&self) -> impl Iterator<Item = &T> {
        std::iter::once(&self.value).chain(&self.also)
    }
}

impl<T: PartialEq> Choice<T> {
    fn holds(&self, value: &T) -> bool {
        self.values().any(|held| held == value)
    }
}

/// What a setting's control shows right now.
#[derive(Clone, Debug, PartialEq)]
pub enum ControlState {
    Toggle(bool),
    Segmented {
        labels: Vec<SharedString>,
        selected: Option<usize>,
    },
    Select {
        label: SharedString,
        options: Vec<SharedString>,
        selected: Option<usize>,
    },
    Stepper {
        label: SharedString,
        can_decrease: bool,
        can_increase: bool,
    },
}

impl<T: Clone + PartialEq + Serialize + DeserializeOwned + Sync> Setting<T> {
    /// The user's value, or the default when they haven't set one or it isn't valid.
    pub fn get(&self, cx: &App) -> T {
        cx.try_global::<Settings>()
            .and_then(|settings| settings.value(self.key))
            .and_then(|value| value.clone().try_into().ok())
            .unwrap_or_else(|| self.default.clone())
    }

    /// Changes the value. Setting the default removes the key, so the file only holds changes. A
    /// value TOML can't hold, such as `None`, removes it too, which leaves the default.
    pub fn set(&self, value: T, cx: &mut App) {
        let value = (value != self.default)
            .then(|| toml::Value::try_from(value).ok())
            .flatten();
        cx.default_global::<Settings>();
        Settings::update_global(cx, |settings, _| settings.edit(self.key, value));
    }

    /// What a control shows for `value`: a choice's label, or the value as written.
    fn label(&self, value: &T, cx: &App) -> SharedString {
        match self
            .choices(cx)
            .into_iter()
            .find(|choice| choice.holds(value))
        {
            Some(choice) => choice.label,
            None => match toml::Value::try_from(value) {
                Ok(toml::Value::String(text)) => text.into(),
                Ok(value) => value.to_string().into(),
                Err(_) => SharedString::default(),
            },
        }
    }

    fn choices(&self, cx: &App) -> Vec<Choice<T>> {
        match self.control {
            Control::Switch(choices) => choices(cx).into(),
            Control::Segmented(choices) | Control::Select(choices) | Control::Stepper(choices) => {
                choices(cx)
            }
        }
    }

    /// Moves `step` choices over, around the ends when `wrap` is set.
    fn move_by(&self, step: isize, wrap: bool, cx: &mut App) {
        let value = self.get(cx);
        let choices = self.choices(cx);
        if choices.is_empty() {
            return;
        }
        let last = choices.len() as isize - 1;
        let index = match choices.iter().position(|choice| choice.holds(&value)) {
            Some(index) if wrap => (index as isize + step).rem_euclid(last + 1),
            Some(index) => (index as isize + step).clamp(0, last),
            None => 0,
        };
        self.choose(index as usize, cx);
    }
}

/// A setting of any type, for listing settings and changing them from a control.
pub trait AnySetting: Sync {
    fn key(&self) -> &'static str;
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn state(&self, cx: &App) -> ControlState;
    /// Flips a toggle, or moves `step` choices over. A stepper stops at either end, and the others
    /// wrap around.
    fn step(&self, step: isize, cx: &mut App);
    /// Like [`AnySetting::step`], but every control stops at the first and last choice.
    fn step_clamped(&self, step: isize, cx: &mut App);
    /// Picks the choice at `index`.
    fn choose(&self, index: usize, cx: &mut App);
    /// The default's label, when the setting holds something else.
    fn changed_from(&self, cx: &App) -> Option<SharedString>;
    /// Goes back to the default, which removes the key from the file.
    fn reset(&self, cx: &mut App);
    /// Whether the file can give this setting `value`, and why not.
    fn check(&self, value: &toml::Value, cx: &App) -> Result<(), String>;
}

impl<T: Clone + PartialEq + Serialize + DeserializeOwned + Sync> AnySetting for Setting<T> {
    fn key(&self) -> &'static str {
        self.key
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn description(&self) -> &'static str {
        self.description
    }

    fn state(&self, cx: &App) -> ControlState {
        let value = self.get(cx);
        let choices = self.choices(cx);
        let selected = choices.iter().position(|choice| choice.holds(&value));
        match self.control {
            Control::Switch(_) => ControlState::Toggle(selected == Some(1)),
            Control::Segmented(_) => ControlState::Segmented {
                labels: choices.into_iter().map(|choice| choice.label).collect(),
                selected,
            },
            Control::Select(_) => ControlState::Select {
                label: self.label(&value, cx),
                options: choices.into_iter().map(|choice| choice.label).collect(),
                selected,
            },
            Control::Stepper(_) => ControlState::Stepper {
                label: self.label(&value, cx),
                can_decrease: selected.is_some_and(|index| index > 0),
                can_increase: selected.is_some_and(|index| index + 1 < choices.len()),
            },
        }
    }

    fn step(&self, step: isize, cx: &mut App) {
        let wrap = !matches!(self.control, Control::Stepper(_));
        self.move_by(step, wrap, cx);
    }

    fn step_clamped(&self, step: isize, cx: &mut App) {
        self.move_by(step, false, cx);
    }

    fn choose(&self, index: usize, cx: &mut App) {
        if let Some(choice) = self.choices(cx).into_iter().nth(index) {
            self.set(choice.value, cx);
        }
    }

    fn changed_from(&self, cx: &App) -> Option<SharedString> {
        (self.get(cx) != self.default).then(|| self.label(&self.default, cx))
    }

    fn reset(&self, cx: &mut App) {
        self.set(self.default.clone(), cx);
    }

    fn check(&self, value: &toml::Value, cx: &App) -> Result<(), String> {
        let value: T = value
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| error.message().to_string())?;
        if matches!(self.control, Control::Select(_)) {
            return Ok(());
        }
        let choices = self.choices(cx);
        if choices.iter().any(|choice| choice.holds(&value)) {
            return Ok(());
        }
        let accepted: Vec<String> = choices
            .iter()
            .flat_map(Choice::values)
            .filter_map(|value| toml::Value::try_from(value).ok())
            .map(|value| value.to_string())
            .collect();
        Err(format!("expected one of {}", accepted.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    static STEPS: Setting<u32> = Setting {
        key: "steps",
        name: "Steps",
        description: "",
        default: 2,
        control: Control::Stepper(|_| {
            (1..=3)
                .map(|step| Choice::new(step, step.to_string()))
                .collect()
        }),
    };

    static FONT: Setting<Option<SharedString>> = Setting {
        key: "font",
        name: "Font",
        description: "",
        default: None,
        control: Control::Select(|_| vec![Choice::new(None, "From the theme")]),
    };

    #[gpui::test]
    fn a_select_shows_a_value_outside_its_choices_as_written(cx: &mut TestAppContext) {
        cx.update(|cx| {
            FONT.set(Some("Fira Code".into()), cx);
            assert_eq!(
                FONT.state(cx),
                ControlState::Select {
                    label: "Fira Code".into(),
                    options: vec!["From the theme".into()],
                    selected: None,
                }
            );
        });
    }

    static SHOWN: Setting<bool> = Setting {
        key: "shown",
        name: "Shown",
        description: "",
        default: true,
        control: Control::Toggle,
    };

    #[gpui::test]
    fn a_toggle_flips_between_its_two_choices(cx: &mut TestAppContext) {
        cx.update(|cx| {
            assert_eq!(SHOWN.state(cx), ControlState::Toggle(true));
            SHOWN.step(1, cx);
            assert_eq!(SHOWN.state(cx), ControlState::Toggle(false));
            assert_eq!(SHOWN.changed_from(cx), Some("On".into()));
            SHOWN.step(-1, cx);
            assert!(SHOWN.get(cx));
        });
    }

    static LEVEL: Setting<u32> = Setting {
        key: "level",
        name: "Level",
        description: "",
        default: 5,
        control: Control::Switch(|_| [Choice::new(1, "Low"), Choice::new(2, "High")]),
    };

    #[gpui::test]
    fn a_switch_over_numbers_starts_from_off_when_its_default_isnt_listed(cx: &mut TestAppContext) {
        cx.update(|cx| {
            assert_eq!(LEVEL.state(cx), ControlState::Toggle(false));
            LEVEL.step(1, cx);
            assert_eq!(LEVEL.get(cx), 1);
            LEVEL.step(1, cx);
            assert_eq!(LEVEL.state(cx), ControlState::Toggle(true));
        });
    }

    static LIMIT: Setting<Option<u32>> = Setting {
        key: "limit",
        name: "Limit",
        description: "",
        default: Some(3),
        control: Control::Stepper(|_| vec![Choice::new(None, "None"), Choice::new(Some(3), "3")]),
    };

    #[gpui::test]
    fn setting_none_over_a_default_goes_back_to_the_default(cx: &mut TestAppContext) {
        cx.update(|cx| {
            LIMIT.set(Some(9), cx);
            LIMIT.set(None, cx);
            assert_eq!(LIMIT.get(cx), Some(3));
            assert_eq!(cx.global::<Settings>().source(), "");
        });
    }

    static SIZE: Setting<Option<u32>> = Setting {
        key: "size",
        name: "Size",
        description: "",
        default: None,
        control: Control::Stepper(|_| {
            vec![
                Choice::new(Some(1), "1 px"),
                Choice::new(None, "2 px").also(Some(2)),
                Choice::new(Some(3), "3 px"),
            ]
        }),
    };

    #[gpui::test]
    fn a_value_a_choice_also_stands_for_shows_as_that_choice(cx: &mut TestAppContext) {
        cx.update(|cx| {
            SIZE.set(Some(2), cx);
            assert_eq!(
                SIZE.state(cx),
                ControlState::Stepper {
                    label: "2 px".into(),
                    can_decrease: true,
                    can_increase: true,
                }
            );
            SIZE.step(1, cx);
            assert_eq!(SIZE.get(cx), Some(3));
            SIZE.step(-1, cx);
            assert_eq!(SIZE.get(cx), None);
            assert!(SIZE.check(&2.into(), cx).is_ok());
            assert_eq!(
                SIZE.check(&4.into(), cx),
                Err("expected one of 1, 2, 3".to_string())
            );
        });
    }

    #[gpui::test]
    fn a_stepper_stops_at_either_end(cx: &mut TestAppContext) {
        cx.update(|cx| {
            STEPS.step(1, cx);
            assert_eq!(STEPS.get(cx), 3);
            assert_eq!(
                STEPS.state(cx),
                ControlState::Stepper {
                    label: "3".into(),
                    can_decrease: true,
                    can_increase: false,
                }
            );
            STEPS.step(1, cx);
            assert_eq!(STEPS.get(cx), 3);
            STEPS.step(-5, cx);
            assert_eq!(STEPS.get(cx), 1);
        });
    }
}
