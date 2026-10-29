//! Fills an empty data folder with the tasks the screenshots in `docs/screenshots` show:
//!
//! ```text
//! cargo run --example demo -- <empty data folder>
//! ```
//!
//! The dates are counted from today, so the Today view always has something overdue and something due.
//! A folder that holds tasks already is left alone.

use chrono::{Datelike, Duration, Local, NaiveDate};
use notes_core::{DueWindow, FilterSpec, Freq, NewTask, Priority, Repeat, Scope, Store};

fn repeat(freq: Freq, interval: u32) -> Repeat {
    Repeat {
        freq,
        interval,
        weekdays: vec![],
        monthday: None,
        nth: None,
        nth_weekday: None,
        from_done: false,
        count: None,
        until: None,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).ok_or("usage: demo <empty data folder>")?;
    let store = Store::open(dir.clone())?;
    if !store.tasks(Scope::All)?.is_empty() {
        return Err("the folder holds tasks already".into());
    }

    let today = Local::now().date_naive();
    let day = |offset: i64| (today + Duration::days(offset)).format("%Y-%m-%d").to_string();
    let monday = |after: NaiveDate| after + Duration::days(7 - i64::from(after.weekday().num_days_from_monday()));
    let add = |title: &str,
               list: Option<&str>,
               parent: Option<&str>,
               due: Option<String>,
               priority: Priority,
               tags: &[&str]| {
        store.create_task(NewTask {
            title: title.into(),
            list_id: list.map(String::from),
            parent_id: parent.map(String::from),
            notes: String::new(),
            start: None,
            due,
            priority: Some(priority),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            ..NewTask::default()
        })
    };

    let work = store.create_list("Работа".into())?;
    store.set_list_color(work.id.clone(), "#2f7bf6".into())?;
    let home = store.create_list("Дом".into())?;
    store.set_list_color(home.id.clone(), "#34c759".into())?;
    let trip = store.create_list("Поездка в Казань".into())?;
    store.set_list_color(trip.id.clone(), "#ff9500".into())?;

    // Work: a task with everything on it, a repeat, something overdue and something plain.
    let report = add(
        "Квартальный отчёт",
        Some(&work.id),
        None,
        Some(format!("{}T17:00", day(0))),
        Priority::High,
        &["отчёты"],
    )?;
    store.set_notes(
        report.id.clone(),
        "Цифры взять из выгрузки за сентябрь, сверить с **бухгалтерией**.\n\n- [x] запросить выгрузку\n- [ ] сверить итоги".into(),
    )?;
    let figures = add("Собрать цифры", None, Some(&report.id), None, Priority::None, &[])?;
    add(
        "Согласовать с финансами",
        None,
        Some(&report.id),
        None,
        Priority::None,
        &[],
    )?;
    add(
        "Отправить руководителю",
        None,
        Some(&report.id),
        None,
        Priority::None,
        &[],
    )?;
    store.complete_task(figures.id)?;
    let plan = std::env::temp_dir().join("план.txt");
    std::fs::write(&plan, "1. Выручка\n2. Расходы\n3. Прогноз\n")?;
    store.add_attachment(report.id.clone(), plan.to_string_lossy().into_owned(), None)?;
    let _ = std::fs::remove_file(&plan);

    let meeting = add(
        "Планёрка",
        Some(&work.id),
        None,
        Some(format!("{}T10:00", monday(today).format("%Y-%m-%d"))),
        Priority::None,
        &["команда"],
    )?;
    store.set_repeat(meeting.id, Some(repeat(Freq::Weekly, 1)))?;
    add(
        "Ответить на письмо подрядчика",
        Some(&work.id),
        None,
        Some(day(-1)),
        Priority::Medium,
        &[],
    )?;
    add("Ревью макетов", Some(&work.id), None, Some(day(3)), Priority::None, &[])?;
    let dropped = add(
        "Обновить презентацию для выставки",
        Some(&work.id),
        None,
        None,
        Priority::None,
        &[],
    )?;
    store.wont_do_task(dropped.id)?;
    let sent = add(
        "Отправить счёт заказчику",
        Some(&work.id),
        None,
        Some(day(-2)),
        Priority::None,
        &[],
    )?;
    store.complete_task(sent.id)?;

    // Home.
    let flowers = add(
        "Полить цветы",
        Some(&home.id),
        None,
        Some(day(0)),
        Priority::None,
        &["дом"],
    )?;
    store.set_repeat(flowers.id, Some(repeat(Freq::Daily, 3)))?;
    add(
        "Оплатить интернет",
        Some(&home.id),
        None,
        Some(day(5)),
        Priority::Low,
        &[],
    )?;
    add(
        "Купить продукты",
        Some(&home.id),
        None,
        None,
        Priority::None,
        &["покупки"],
    )?;

    // The trip.
    add(
        "Купить билеты",
        Some(&trip.id),
        None,
        Some(day(2)),
        Priority::Medium,
        &[],
    )?;
    add(
        "Забронировать гостиницу",
        Some(&trip.id),
        None,
        Some(day(6)),
        Priority::None,
        &[],
    )?;

    // Inbox: two loose tasks and a project.
    add("Записаться к стоматологу", None, None, None, Priority::None, &[])?;
    add("Позвонить в банк", None, None, None, Priority::None, &[])?;
    let site = add("Запуск сайта", None, None, None, Priority::None, &[])?;
    store.set_project(site.id.clone(), true)?;
    add("Тексты", None, Some(&site.id), Some(day(4)), Priority::None, &[])?;
    add("Макеты", None, Some(&site.id), Some(day(1)), Priority::Medium, &[])?;

    store.create_filter(
        "Ближайшие 7 дней".into(),
        FilterSpec {
            due: DueWindow::Next { days: 7 },
            ..FilterSpec::default()
        },
    )?;

    println!("{dir}: {} open tasks", store.tasks(Scope::All)?.len());
    Ok(())
}
