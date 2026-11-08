//! System tray icons through systray-util: it stands in
//! for the taskbar's tray so apps send their icons here. Runs on its own thread.

use std::sync::{mpsc, Arc, Mutex, OnceLock};
use systray_util::{StableId, Systray, SystrayEvent, SystrayIconAction};

#[derive(Clone, Debug, PartialEq)]
pub struct Icon {
    pub id: String,
    pub tooltip: String,
    /// Straight RGBA pixels, width, height.
    pub rgba: Option<Arc<(Vec<u8>, u32, u32)>>,
}

static ACT: OnceLock<Mutex<mpsc::Sender<(String, SystrayIconAction)>>> = OnceLock::new();

pub enum Click { Left, Right, Middle, Double }

pub fn click(id: &str, how: Click) {
    let a = match how { Click::Left => SystrayIconAction::LeftClick, Click::Right => SystrayIconAction::RightClick,
        Click::Middle => SystrayIconAction::MiddleClick, Click::Double => SystrayIconAction::LeftDoubleClick };
    if let Some(Ok(t)) = ACT.get().map(|m| m.lock()) { let _ = t.send((id.to_string(), a)); }
}

pub fn click_btn(id: &str, b: crate::win::Btn) {
    use crate::win::Btn;
    click(id, match b { Btn::Left => Click::Left, Btn::Right => Click::Right, Btn::Middle => Click::Middle, Btn::Double => Click::Double });
}

pub fn start(on: impl Fn(Vec<Icon>) + Send + 'static) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, SystrayIconAction)>();
    // std sender for the UI thread, forwarded into the tokio channel the tray loop waits on
    let (stx, srx) = mpsc::channel::<(String, SystrayIconAction)>();
    let _ = ACT.set(Mutex::new(stx));
    std::thread::spawn(move || for m in srx { if tx.send(m).is_err() { return; } });
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
        rt.block_on(async move {
            let Ok(mut tray) = Systray::new() else { return };
            let mut order: Vec<String> = vec![];
            loop {
                tokio::select! {
                    ev = tray.events() => {
                        let Some(ev) = ev else { return };
                        match &ev {
                            SystrayEvent::IconAdd(i) => { let id = i.stable_id.to_string(); if !order.contains(&id) { order.push(id); } }
                            SystrayEvent::IconRemove(id) => { let id = id.to_string(); order.retain(|x| *x != id); }
                            SystrayEvent::IconUpdate(_) => {}
                        }
                        let icons = tray.icons();
                        on(order.iter().filter_map(|id| icons.iter().find(|i| i.stable_id.to_string() == *id)).map(|i| Icon {
                            id: i.stable_id.to_string(),
                            tooltip: i.tooltip.clone(),
                            rgba: i.icon_image.as_ref().map(|im| Arc::new((im.as_raw().clone(), im.width(), im.height()))),
                        }).collect());
                    }
                    act = rx.recv() => {
                        let Some((id, a)) = act else { return };
                        if let Ok(sid) = id.parse::<StableId>() { let _ = tray.send_action(&sid, &a); }
                    }
                }
            }
        });
    });
}
