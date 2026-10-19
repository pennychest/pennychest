//! The dashboard: which widgets it shows, in what order, with what settings. Saved on the
//! server so it's the same on every device.

use serde::{Deserialize, Serialize};
use sqlx::SqliteExecutor;

use crate::settings::{Json, Setting, SettingError};

const LAYOUT: Setting<Json<Layout>> = Setting::new("dashboard.layout");

const MAX_WIDGETS: usize = 40;
const MAX_ID_CHARS: usize = 40;
const MONTHS: std::ops::RangeInclusive<u32> = 3..=36;
const MAX_TREND_CATEGORIES: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub widgets: Vec<Widget>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Widget {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: WidgetType,
    #[serde(default)]
    pub settings: WidgetSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetType {
    Stat,
    SpendingByCategory,
    SpendingTrend,
    IncomeVsExpenses,
    NetWorth,
    Budgets,
    TopMerchants,
    Pending,
    UnmatchedTaps,
    Subscriptions,
}

/// Every widget has the same settings, each used by some kinds of widget. Unset ones are
/// written out as null.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WidgetSettings {
    /// Stat tiles: which figure.
    #[serde(default)]
    pub stat: Option<Stat>,
    /// Widgets over a period.
    #[serde(default)]
    pub period: Option<Period>,
    /// Month-by-month charts: how many months.
    #[serde(default)]
    pub months: Option<u32>,
    #[serde(default)]
    pub chart: Option<Chart>,
    /// Spending trend: categories to plot as their own series (none means all spending).
    #[serde(default)]
    pub categories: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stat {
    NetWorth,
    Income,
    Expenses,
    SavingsRate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    ThisMonth,
    LastMonth,
    Last3Months,
    Last12Months,
    ThisYear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Chart {
    Bar,
    Donut,
    Line,
}

#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Setting(#[from] SettingError),
}

impl Layout {
    /// What a new install shows.
    pub fn default_layout() -> Self {
        let widget = |id: &str, kind, settings| Widget {
            id: id.into(),
            kind,
            settings,
        };
        let stat = |stat| WidgetSettings {
            stat: Some(stat),
            ..WidgetSettings::default()
        };
        Self {
            widgets: vec![
                widget("net-worth", WidgetType::Stat, stat(Stat::NetWorth)),
                widget("income", WidgetType::Stat, stat(Stat::Income)),
                widget("expenses", WidgetType::Stat, stat(Stat::Expenses)),
                widget("savings-rate", WidgetType::Stat, stat(Stat::SavingsRate)),
                widget(
                    "by-category",
                    WidgetType::SpendingByCategory,
                    WidgetSettings {
                        period: Some(Period::ThisMonth),
                        ..WidgetSettings::default()
                    },
                ),
                widget("pending", WidgetType::Pending, WidgetSettings::default()),
                widget(
                    "in-out",
                    WidgetType::IncomeVsExpenses,
                    WidgetSettings {
                        months: Some(12),
                        ..WidgetSettings::default()
                    },
                ),
            ],
        }
    }

    /// Checks the limits that the types can't express.
    pub fn validate(&self) -> Result<(), LayoutError> {
        let invalid = |message: String| Err(LayoutError::Invalid(message));
        if self.widgets.len() > MAX_WIDGETS {
            return invalid(format!(
                "A dashboard can have at most {MAX_WIDGETS} widgets."
            ));
        }
        for widget in &self.widgets {
            let id_chars = widget.id.chars().count();
            if id_chars == 0 || id_chars > MAX_ID_CHARS {
                return invalid(format!(
                    "A widget's id must be 1 to {MAX_ID_CHARS} characters long."
                ));
            }
            if let Some(months) = widget.settings.months
                && !MONTHS.contains(&months)
            {
                return invalid(format!(
                    "A chart can cover {} to {} months, not {months}.",
                    MONTHS.start(),
                    MONTHS.end()
                ));
            }
            if let Some(categories) = &widget.settings.categories
                && categories.len() > MAX_TREND_CATEGORIES
            {
                return invalid(format!(
                    "A spending trend can show at most {MAX_TREND_CATEGORIES} categories."
                ));
            }
        }
        Ok(())
    }
}

/// The saved layout, or the default one.
pub async fn layout(db: impl SqliteExecutor<'_>) -> Result<Layout, LayoutError> {
    Ok(LAYOUT
        .get(db)
        .await?
        .map_or_else(Layout::default_layout, |Json(layout)| layout))
}

pub async fn save_layout(
    db: impl SqliteExecutor<'_>,
    layout: Layout,
) -> Result<Layout, LayoutError> {
    layout.validate()?;
    let layout = Json(layout);
    LAYOUT.set(db, &layout).await?;
    Ok(layout.0)
}

/// Goes back to the default dashboard.
pub async fn reset_layout(db: impl SqliteExecutor<'_>) -> Result<Layout, LayoutError> {
    LAYOUT.clear(db).await?;
    Ok(Layout::default_layout())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use serde_json::json;

    fn parse(value: serde_json::Value) -> Result<Layout, String> {
        let layout: Layout = serde_json::from_value(value).map_err(|e| e.to_string())?;
        layout.validate().map_err(|e| e.to_string())?;
        Ok(layout)
    }

    #[test]
    fn writes_unset_settings_as_null() {
        let layout = Layout::default_layout();
        assert_eq!(
            serde_json::to_value(&layout.widgets[0]).unwrap(),
            json!({
                "id": "net-worth",
                "type": "stat",
                "settings": {
                    "stat": "net_worth", "period": null, "months": null,
                    "chart": null, "categories": null
                }
            })
        );
    }

    #[test]
    fn accepts_good_layouts() {
        let layout = parse(json!({"widgets": [
            {"id": "a", "type": "net_worth", "settings": {"months": 24}},
            {"id": "b", "type": "spending_trend",
             "settings": {"months": 12, "chart": "line", "categories": ["Expenses:Food"]}},
            {"id": "c", "type": "unmatched_taps"},
            {"id": "d", "type": "pending", "settings": {"stat": null}}
        ]}))
        .unwrap();
        assert_eq!(layout.widgets.len(), 4);
        assert_eq!(layout.widgets[2].settings, WidgetSettings::default());
    }

    #[test]
    fn refuses_bad_layouts() {
        let too_many: Vec<_> = (0..41)
            .map(|i| json!({"id": i.to_string(), "type": "pending"}))
            .collect();
        for bad in [
            json!({"widgets": [{"id": "a", "type": "pie_of_everything"}]}),
            json!({"widgets": [{"id": "a", "type": "stat", "settings": {"stat": "mood"}}]}),
            json!({"widgets": [{"id": "a", "type": "net_worth", "settings": {"colour": "red"}}]}),
            json!({"widgets": [{"id": "a", "type": "net_worth", "settings": {"months": 1000}}]}),
            json!({"widgets": [{"id": "a", "type": "net_worth", "settings": {"months": 2}}]}),
            json!({"widgets": [{"id": "", "type": "pending"}]}),
            json!({"widgets": [{"id": "x".repeat(41), "type": "pending"}]}),
            json!({"widgets": [{"id": "a", "type": "spending_trend",
                                "settings": {"categories": ["1","2","3","4","5","6","7","8","9"]}}]}),
            json!({"widgets": too_many}),
            json!({}),
        ] {
            assert!(parse(bad.clone()).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn defaults_saves_and_resets() {
        let pool = db::open_in_memory().await.unwrap();
        assert_eq!(layout(&pool).await.unwrap(), Layout::default_layout());

        let mine = parse(json!({"widgets": [{"id": "a", "type": "budgets"}]})).unwrap();
        save_layout(&pool, mine.clone()).await.unwrap();
        assert_eq!(layout(&pool).await.unwrap(), mine);

        assert_eq!(reset_layout(&pool).await.unwrap(), Layout::default_layout());
        assert_eq!(layout(&pool).await.unwrap(), Layout::default_layout());
    }

    #[tokio::test]
    async fn reads_a_layout_with_only_some_settings() {
        let pool = db::open_in_memory().await.unwrap();
        sqlx::query(
            r#"INSERT INTO app_settings (key, value) VALUES ('dashboard.layout',
               '{"widgets": [{"id": "a", "type": "pending", "settings": {"months": 3}}]}')"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let saved = layout(&pool).await.unwrap();
        assert_eq!(saved.widgets[0].settings.months, Some(3));
    }
}
