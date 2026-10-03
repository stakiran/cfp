#![cfg_attr(not(test), windows_subsystem = "windows")]

mod collector;
mod store;

use collector::{Collector, MouseEvent, Record};
use std::cell::RefCell;
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use store::Store;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::Input::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_TRAY: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;
const MENU_EXIT: usize = 1;
/// ダブルクリックとみなす移動量の上限（Raw Input の単位。DPI により変わるので目安）
const DBL_DIST: f64 = 20.0;

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

struct App {
    collector: Collector,
    store: Store,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// コレクタを操作し、確定したバケットがあれば保存する
fn with_collector(f: impl FnOnce(&mut Collector) -> Option<Record>) {
    APP.with(|cell| {
        // メニュー表示中などの再入時は借用できないので、そのイベントは捨てる
        let Ok(mut guard) = cell.try_borrow_mut() else { return };
        let Some(app) = guard.as_mut() else { return };
        if let Some(rec) = f(&mut app.collector) {
            let _ = app.store.insert(&rec);
        }
    });
}

/// exe と同じフォルダ（スタートアップから起動しても作業ディレクトリに左右されない）
fn data_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn fatal(msg: &str) -> ! {
    let text: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(None, PCWSTR(text.as_ptr()), w!("cfp"), MB_ICONERROR);
    }
    std::process::exit(1);
}

fn main() {
    unsafe {
        // 多重起動防止
        let _mutex = CreateMutexW(None, true, w!("Local\\cfp-collector"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }

        let dir = data_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            fatal(&format!("データフォルダを作成できません: {e}"));
        }
        let store = match Store::open(&dir.join("cfp.db")) {
            Ok(s) => s,
            Err(e) => fatal(&format!("DB を開けません: {e}")),
        };
        APP.with(|cell| {
            *cell.borrow_mut() = Some(App {
                collector: Collector::new(GetDoubleClickTime() as u64, DBL_DIST),
                store,
            });
        });

        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);

        let hinst = GetModuleHandleW(None).unwrap_or_else(|_| fatal("GetModuleHandleW 失敗"));
        let class = w!("cfp_collector");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);

        // Raw Input の受け口。表示はしないが通常のトップレベルウィンドウにする
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("cfp"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            hinst,
            None,
        )
        .unwrap_or_else(|_| fatal("ウィンドウを作成できません"));

        let devices = [
            // 汎用デスクトップ / キーボード
            RAWINPUTDEVICE {
                usUsagePage: 0x01,
                usUsage: 0x06,
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: hwnd,
            },
            // 汎用デスクトップ / マウス
            RAWINPUTDEVICE {
                usUsagePage: 0x01,
                usUsage: 0x02,
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: hwnd,
            },
        ];
        if RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32).is_err() {
            fatal("Raw Input を登録できません");
        }

        SetTimer(hwnd, TIMER_ID, 1000, None);
        add_tray_icon(hwnd);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        remove_tray_icon(hwnd);
        // 終了時の途中バケットは保存しない
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_INPUT => {
                handle_raw_input(HRAWINPUT(lparam.0 as _));
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_TIMER => {
                with_collector(|c| c.tick(now_ms()));
                LRESULT(0)
            }
            WM_TRAY => {
                let event = (lparam.0 as u32) & 0xFFFF;
                if event == WM_RBUTTONUP || event == WM_LBUTTONUP {
                    show_menu(hwnd);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::Relaxed) => {
                // Explorer 再起動時にトレイアイコンを復元
                add_tray_icon(hwnd);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

unsafe fn handle_raw_input(h: HRAWINPUT) {
    let mut raw = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    let n = GetRawInputData(
        h,
        RID_INPUT,
        Some(&mut raw as *mut RAWINPUT as *mut _),
        &mut size,
        size_of::<RAWINPUTHEADER>() as u32,
    );
    if n == u32::MAX || n == 0 {
        return;
    }
    let t = now_ms();

    if raw.header.dwType == RIM_TYPEKEYBOARD.0 {
        let kb = raw.data.keyboard;
        const RI_KEY_BREAK_: u16 = 0x01;
        const RI_KEY_E0_: u16 = 0x02;
        // NumLock 時などに挿入される偽の Shift を除外
        if kb.Flags & RI_KEY_E0_ != 0 && (kb.MakeCode == 0x2A || kb.MakeCode == 0x36) {
            return;
        }
        let is_up = kb.Flags & RI_KEY_BREAK_ != 0;
        with_collector(|c| c.on_key(kb.VKey, is_up, t));
    } else if raw.header.dwType == RIM_TYPEMOUSE.0 {
        let m = raw.data.mouse;
        const MOUSE_MOVE_ABSOLUTE_: u16 = 0x01;
        // 絶対座標（リモートデスクトップ、ペンタブレット等）は移動量に数えない
        let absolute = m.usFlags.0 & MOUSE_MOVE_ABSOLUTE_ != 0;
        let buttons = m.Anonymous.Anonymous;
        let ev = MouseEvent {
            dx: if absolute { 0 } else { m.lLastX },
            dy: if absolute { 0 } else { m.lLastY },
            button_flags: buttons.usButtonFlags,
            button_data: buttons.usButtonData as i16,
        };
        with_collector(|c| c.on_mouse(ev, t));
    }
}

unsafe fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    }
}

unsafe fn add_tray_icon(hwnd: HWND) {
    let mut nid = tray_data(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = LoadIconW(None, IDI_APPLICATION).unwrap_or_default();
    for (dst, src) in nid.szTip.iter_mut().zip("cfp: 記録中".encode_utf16()) {
        *dst = src;
    }
    let _ = Shell_NotifyIconW(NIM_ADD, &nid);
}

unsafe fn remove_tray_icon(hwnd: HWND) {
    let nid = tray_data(hwnd);
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
}

unsafe fn show_menu(hwnd: HWND) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("終了"));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON,
        pt.x,
        pt.y,
        0,
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
    if cmd.0 as usize == MENU_EXIT {
        let _ = DestroyWindow(hwnd);
    }
}
