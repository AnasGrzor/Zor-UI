//! OLE file drops provide enter/over/leave events that WM_DROPFILES cannot deliver.
//! Callbacks only publish hover state or queue work; they never reborrow the app.
use crate::{app, gfx::R, localsend};
use std::{cell::Cell, path::PathBuf, sync::{Arc, Mutex}};
use windows::{core::{implement, Result}, Win32::{
    Foundation::{HWND, POINT, POINTL}, Graphics::Gdi::ScreenToClient,
    System::{Com::{IDataObject, FORMATETC, DVASPECT_CONTENT, TYMED_HGLOBAL},
        Ole::{IDropTarget, IDropTarget_Impl, RegisterDragDrop, RevokeDragDrop, ReleaseStgMedium,
            CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE}, SystemServices::MODIFIERKEYS_FLAGS},
    UI::{HiDpi::GetDpiForWindow, Shell::{DragQueryFileW, HDROP}},
}};

pub struct Registration { hwnd: HWND, regions: Arc<Mutex<Vec<R>>> }
impl Registration {
    pub fn new(hwnd: HWND) -> Result<Self> {
        let regions = Arc::new(Mutex::new(Vec::new()));
        let target: IDropTarget = Target { hwnd, regions: regions.clone(), files: Cell::new(false), hovering: Cell::new(false) }.into();
        unsafe { RegisterDragDrop(hwnd, &target)?; }
        Ok(Self { hwnd, regions })
    }
    pub fn update(&self, regions: Vec<R>) { *self.regions.lock().unwrap() = regions; }
}
impl Drop for Registration {
    fn drop(&mut self) { unsafe { let _ = RevokeDragDrop(self.hwnd); } }
}

fn format() -> FORMATETC {
    FORMATETC { cfFormat: CF_HDROP.0, dwAspect: DVASPECT_CONTENT.0, lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32, ..Default::default() }
}
fn inside(regions: &[R], x: f32, y: f32) -> bool { regions.iter().any(|r| r.contains(x, y)) }

#[implement(IDropTarget)]
struct Target { hwnd: HWND, regions: Arc<Mutex<Vec<R>>>, files: Cell<bool>, hovering: Cell<bool> }
impl Target {
    fn hover(&self, on: bool) {
        if self.hovering.replace(on) != on { localsend::set_drag_hover(on); }
    }
    fn over(&self, pt: &POINTL, effect: *mut DROPEFFECT) -> bool {
        let mut point = POINT { x: pt.x, y: pt.y };
        let scale = unsafe { GetDpiForWindow(self.hwnd) } as f32 / 96.0;
        let client = unsafe { ScreenToClient(self.hwnd, &mut point).as_bool() };
        let copy = !effect.is_null() && unsafe { (*effect).0 & DROPEFFECT_COPY.0 != 0 };
        let on = self.files.get() && copy && client && scale > 0.0 && !localsend::status().busy
            && inside(&self.regions.lock().unwrap(), point.x as f32 / scale, point.y as f32 / scale);
        if !effect.is_null() { unsafe { *effect = if on { DROPEFFECT_COPY } else { DROPEFFECT_NONE }; } }
        self.hover(on);
        on
    }
}
impl Drop for Target {
    fn drop(&mut self) { self.hover(false); }
}
#[allow(non_snake_case)]
impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, data: Option<&IDataObject>, _: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        self.files.set(data.is_some_and(|d| unsafe { d.QueryGetData(&format()).is_ok() }));
        self.over(pt, effect);
        Ok(())
    }
    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        self.over(pt, effect); Ok(())
    }
    fn DragLeave(&self) -> Result<()> { self.files.set(false); self.hover(false); Ok(()) }
    fn Drop(&self, data: Option<&IDataObject>, _: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let accepted = self.over(pt, effect);
        self.hover(false); self.files.set(false);
        if !accepted { return Ok(()); }
        let paths = data.map(read_files).transpose()?.unwrap_or_default();
        if paths.is_empty() {
            if !effect.is_null() { unsafe { *effect = DROPEFFECT_NONE; } }
        } else {
            app::post(Box::new(move |cx| localsend::drop_files(paths, cx)));
        }
        Ok(())
    }
}

fn read_files(data: &IDataObject) -> Result<Vec<PathBuf>> {
    use std::os::windows::ffi::OsStringExt;
    unsafe {
        let mut medium = data.GetData(&format())?;
        if medium.tymed != TYMED_HGLOBAL.0 as u32 {
            ReleaseStgMedium(&mut medium);
            return Err(windows::Win32::Foundation::E_FAIL.into());
        }
        let drop = HDROP(medium.u.hGlobal.0);
        let paths = (0..DragQueryFileW(drop, u32::MAX, None)).map(|i| {
            let n = DragQueryFileW(drop, i, None);
            let mut buf = vec![0u16; n as usize + 1];
            DragQueryFileW(drop, i, Some(&mut buf));
            PathBuf::from(std::ffi::OsString::from_wide(&buf[..n as usize]))
        }).collect();
        // The data object owns the HDROP. DragFinish would free it a second time.
        ReleaseStgMedium(&mut medium);
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_island_region_accepts_a_drag() {
        let regions = [R::new(450.0, 8.0, 200.0, 32.0)];
        assert!(inside(&regions, 500.0, 20.0));
        assert!(!inside(&regions, 400.0, 20.0));
        assert!(!inside(&regions, 500.0, 50.0));
        assert!(!inside(&[], 500.0, 20.0));
    }
    #[test]
    fn explorer_data_object_paths_are_read_without_taking_ownership() {
        std::thread::spawn(|| unsafe {
            use windows::{core::HSTRING, Win32::{System::Ole::{OleInitialize, OleUninitialize},
                UI::Shell::{SHCreateItemFromParsingName, IShellItem, BHID_DataObject}}};
            OleInitialize(None).unwrap();
            {
                let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
                let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None).unwrap();
                let data: IDataObject = item.BindToHandler(None, &BHID_DataObject).unwrap();
                assert!(data.QueryGetData(&format()).is_ok());
                // Both reads must succeed: the sender still owns the data after our read.
                assert_eq!(read_files(&data).unwrap(), vec![path.clone()]);
                assert_eq!(read_files(&data).unwrap(), vec![path]);
            }
            OleUninitialize();
        }).join().unwrap();
    }
}
