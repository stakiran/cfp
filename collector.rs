//! 入力イベントを10秒バケットに集計する。Windows API には依存しない。
//! キーコードやカーソル座標は保持せず、分類済みのカウントだけを持つ。

pub const BUCKET_MS: u64 = 10_000;

/// マウス移動イベントの間隔がこれ以下なら「動き続けている」とみなす
const MOVE_GAP_MS: u64 = 100;
/// キーボードイベントがこれだけ途絶えたら押下状態をリセットする
/// （ロック画面や UAC で key-up を取りこぼした場合の保険）
const KEY_STALE_MS: u64 = 3_000;

/// 1バケット分の記録。DB の1行に対応する。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Record {
    /// バケット開始時刻（Unix秒）
    pub t: i64,
    pub key_char: u32,
    pub key_space: u32,
    pub key_bs: u32,
    pub key_enter: u32,
    pub key_tab: u32,
    pub key_nav: u32,
    pub key_shortcut: u32,
    pub key_ime: u32,
    pub key_esc: u32,
    /// 打鍵間隔: <100ms, 100-300ms, 300-1000ms, >=1000ms
    pub iki: [u32; 4],
    pub mouse_dist: f64,
    pub mouse_move_ms: u32,
    pub click_l: u32,
    pub click_r: u32,
    pub click_m: u32,
    pub dblclick: u32,
    pub drag_dist: f64,
    pub wheel_v: f64,
    pub wheel_h: f64,
    pub active_secs: u32,
    pub max_idle_ms: u32,
    pub hand_switches: u32,
}

/// Raw Input のマウスイベントから必要な部分だけ抜き出したもの
#[derive(Debug, Default, Clone, Copy)]
pub struct MouseEvent {
    pub dx: i32,
    pub dy: i32,
    pub button_flags: u16,
    pub button_data: i16,
}

// RI_MOUSE_* フラグ
const LDOWN: u16 = 0x0001;
const LUP: u16 = 0x0002;
const RDOWN: u16 = 0x0004;
const RUP: u16 = 0x0008;
const MDOWN: u16 = 0x0010;
const MUP: u16 = 0x0020;
const WHEEL: u16 = 0x0400;
const HWHEEL: u16 = 0x0800;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Device {
    Keyboard,
    Mouse,
}

enum KeyClass {
    Modifier,
    Char,
    Space,
    Backspace,
    Enter,
    Tab,
    Nav,
    Ime,
    Esc,
    Other,
}

fn classify(vk: u16) -> KeyClass {
    match vk {
        0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5 => KeyClass::Modifier,
        0x08 | 0x2E => KeyClass::Backspace,
        0x09 => KeyClass::Tab,
        0x0D => KeyClass::Enter,
        0x1B => KeyClass::Esc,
        0x20 => KeyClass::Space,
        0x21..=0x28 => KeyClass::Nav,
        // かな, IME ON/OFF, 漢字, 変換, 無変換, 英数, カタカナ/ひらがな, 半角/全角 など
        0x15..=0x1A | 0x1C | 0x1D | 0xF0..=0xF4 => KeyClass::Ime,
        0x30..=0x39 | 0x41..=0x5A | 0x60..=0x6F | 0xBA..=0xC0 | 0xDB..=0xDF | 0xE2 => {
            KeyClass::Char
        }
        _ => KeyClass::Other,
    }
}

pub struct Collector {
    // --- バケット単位の状態 ---
    cur_id: Option<u64>,
    /// 境界から連続して観測できたバケットだけ保存する
    valid: bool,
    rec: Record,
    active_mask: u16,
    last_input_ms: u64,
    max_idle_ms: u64,

    // --- バケットをまたぐ状態 ---
    pressed: [bool; 256],
    last_kb_event_ms: u64,
    last_key_ms: Option<u64>,
    last_move_ms: Option<u64>,
    buttons: u8,
    last_device: Option<Device>,
    last_ldown_ms: Option<u64>,
    dist_since_ldown: f64,
    dbl_time_ms: u64,
    dbl_dist: f64,
}

impl Collector {
    /// dbl_time_ms: ダブルクリック判定時間（GetDoubleClickTime）
    /// dbl_dist: ダブルクリックとみなす移動量の上限（Raw Input の単位）
    pub fn new(dbl_time_ms: u64, dbl_dist: f64) -> Self {
        Self {
            cur_id: None,
            valid: false,
            rec: Record::default(),
            active_mask: 0,
            last_input_ms: 0,
            max_idle_ms: 0,
            pressed: [false; 256],
            last_kb_event_ms: 0,
            last_key_ms: None,
            last_move_ms: None,
            buttons: 0,
            last_device: None,
            last_ldown_ms: None,
            dist_since_ldown: 0.0,
            dbl_time_ms,
            dbl_dist,
        }
    }

    /// 定期的に呼ぶ。バケットが確定したら返す。
    pub fn tick(&mut self, now_ms: u64) -> Option<Record> {
        self.roll(now_ms)
    }

    /// vk: 仮想キーコード（分類にのみ使い、保存しない）
    pub fn on_key(&mut self, vk: u16, is_up: bool, now_ms: u64) -> Option<Record> {
        let out = self.roll(now_ms);
        if vk == 0 || vk >= 0xFF {
            return out;
        }
        if now_ms.saturating_sub(self.last_kb_event_ms) > KEY_STALE_MS {
            self.pressed = [false; 256];
        }
        self.last_kb_event_ms = now_ms;

        let i = vk as usize;
        if is_up {
            self.pressed[i] = false;
            return out;
        }
        if self.pressed[i] {
            // 自動リピート: 何も数えない
            return out;
        }
        self.pressed[i] = true;
        self.touch(now_ms, Device::Keyboard);

        let class = classify(vk);
        if matches!(class, KeyClass::Modifier) {
            return out;
        }

        // 打鍵間隔（修飾キー以外）
        if let Some(prev) = self.last_key_ms {
            let d = now_ms.saturating_sub(prev);
            let idx = match d {
                0..=99 => 0,
                100..=299 => 1,
                300..=999 => 2,
                _ => 3,
            };
            self.rec.iki[idx] += 1;
        }
        self.last_key_ms = Some(now_ms);

        let p = &self.pressed;
        let with_mod = p[0x11] || p[0x12] || p[0x5B] || p[0x5C]; // Ctrl, Alt, Win
        let r = &mut self.rec;
        if with_mod {
            r.key_shortcut += 1;
            return out;
        }
        match class {
            KeyClass::Char => r.key_char += 1,
            KeyClass::Space => r.key_space += 1,
            KeyClass::Backspace => r.key_bs += 1,
            KeyClass::Enter => r.key_enter += 1,
            KeyClass::Tab => r.key_tab += 1,
            KeyClass::Nav => r.key_nav += 1,
            KeyClass::Ime => r.key_ime += 1,
            KeyClass::Esc => r.key_esc += 1,
            KeyClass::Modifier | KeyClass::Other => {}
        }
        out
    }

    pub fn on_mouse(&mut self, ev: MouseEvent, now_ms: u64) -> Option<Record> {
        let out = self.roll(now_ms);
        let (dx, dy) = (ev.dx as f64, ev.dy as f64);
        let dist = (dx * dx + dy * dy).sqrt();
        let f = ev.button_flags;
        if dist == 0.0 && f == 0 {
            return out;
        }
        self.touch(now_ms, Device::Mouse);

        if dist > 0.0 {
            self.rec.mouse_dist += dist;
            if let Some(prev) = self.last_move_ms {
                let g = now_ms.saturating_sub(prev);
                if g <= MOVE_GAP_MS {
                    self.rec.mouse_move_ms += g as u32;
                }
            }
            self.last_move_ms = Some(now_ms);
            if self.buttons != 0 {
                self.rec.drag_dist += dist;
            }
            self.dist_since_ldown += dist;
        }

        if f & LDOWN != 0 {
            self.rec.click_l += 1;
            self.buttons |= 1;
            let is_dbl = matches!(self.last_ldown_ms, Some(p)
                if now_ms.saturating_sub(p) <= self.dbl_time_ms
                    && self.dist_since_ldown <= self.dbl_dist);
            if is_dbl {
                self.rec.dblclick += 1;
                self.last_ldown_ms = None; // 3連打を2回と数えない
            } else {
                self.last_ldown_ms = Some(now_ms);
            }
            self.dist_since_ldown = 0.0;
        }
        if f & LUP != 0 {
            self.buttons &= !1;
        }
        if f & RDOWN != 0 {
            self.rec.click_r += 1;
            self.buttons |= 2;
        }
        if f & RUP != 0 {
            self.buttons &= !2;
        }
        if f & MDOWN != 0 {
            self.rec.click_m += 1;
            self.buttons |= 4;
        }
        if f & MUP != 0 {
            self.buttons &= !4;
        }
        let notches = (ev.button_data as f64).abs() / 120.0;
        if f & WHEEL != 0 {
            self.rec.wheel_v += notches;
        }
        if f & HWHEEL != 0 {
            self.rec.wheel_h += notches;
        }
        out
    }

    /// 入力があったことを記録する（活動秒数・最長無入力・持ち替え）
    fn touch(&mut self, now_ms: u64, dev: Device) {
        let start = self.cur_id.unwrap_or(0) * BUCKET_MS;
        let sec = (now_ms.saturating_sub(start) / 1000).min(9);
        self.active_mask |= 1 << sec;
        let gap = now_ms.saturating_sub(self.last_input_ms);
        self.max_idle_ms = self.max_idle_ms.max(gap);
        self.last_input_ms = now_ms;
        if let Some(prev) = self.last_device {
            if prev != dev {
                self.rec.hand_switches += 1;
            }
        }
        self.last_device = Some(dev);
    }

    /// 時刻がバケット境界を越えていたら現在のバケットを確定する
    fn roll(&mut self, now_ms: u64) -> Option<Record> {
        let id = now_ms / BUCKET_MS;
        match self.cur_id {
            Some(c) if c == id => None,
            Some(c) => {
                let contiguous = id == c + 1;
                let out = if self.valid && contiguous {
                    Some(self.finish(c))
                } else {
                    None // スリープ等で途切れたバケットは捨てる
                };
                self.start(id, contiguous);
                out
            }
            None => {
                self.start(id, false); // 起動直後の途中バケットは捨てる
                None
            }
        }
    }

    fn finish(&mut self, id: u64) -> Record {
        let end = (id + 1) * BUCKET_MS;
        let idle = self.max_idle_ms.max(end.saturating_sub(self.last_input_ms));
        let mut r = std::mem::take(&mut self.rec);
        r.t = (id * BUCKET_MS / 1000) as i64;
        r.active_secs = self.active_mask.count_ones();
        r.max_idle_ms = idle.min(BUCKET_MS) as u32;
        r
    }

    fn start(&mut self, id: u64, valid: bool) {
        self.cur_id = Some(id);
        self.valid = valid;
        self.rec = Record::default();
        self.active_mask = 0;
        self.last_input_ms = id * BUCKET_MS;
        self.max_idle_ms = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_700_000_000_000; // バケット境界ちょうど

    fn col() -> Collector {
        let mut c = Collector::new(500, 20.0);
        c.tick(T0 - 1); // 前のバケットで起動
        c
    }

    fn key(c: &mut Collector, vk: u16, t: u64) {
        c.on_key(vk, false, t);
        c.on_key(vk, true, t + 30);
    }

    #[test]
    fn startup_bucket_is_discarded_then_next_is_saved() {
        let mut c = col();
        assert!(c.tick(T0).is_none()); // 起動時の途中バケット
        let r = c.tick(T0 + BUCKET_MS).expect("完全なバケット");
        assert_eq!(r.t, (T0 / 1000) as i64);
        assert_eq!(r.active_secs, 0);
        assert_eq!(r.max_idle_ms, 10_000);
    }

    #[test]
    fn keys_are_classified_and_repeats_ignored() {
        let mut c = col();
        c.tick(T0);
        key(&mut c, 0x41, T0 + 100); // A
        key(&mut c, 0x20, T0 + 250); // Space
        // Backspace 長押し: 押下1回 + リピート多数
        c.on_key(0x08, false, T0 + 1000);
        for i in 1..20 {
            c.on_key(0x08, false, T0 + 1000 + i * 33);
        }
        c.on_key(0x08, true, T0 + 1700);
        // Ctrl+C
        c.on_key(0x11, false, T0 + 3000);
        key(&mut c, 0x43, T0 + 3100);
        c.on_key(0x11, true, T0 + 3200);
        key(&mut c, 0x1C, T0 + 5000); // 変換
        let r = c.tick(T0 + BUCKET_MS).unwrap();
        assert_eq!(r.key_char, 1);
        assert_eq!(r.key_space, 1);
        assert_eq!(r.key_bs, 1);
        assert_eq!(r.key_shortcut, 1);
        assert_eq!(r.key_ime, 1);
        // 間隔: 150ms, 750ms, 2100ms, 1900ms
        assert_eq!(r.iki, [0, 1, 1, 2]);
    }

    #[test]
    fn mouse_aggregation() {
        let mut c = col();
        c.tick(T0);
        let mv = |dx, dy| MouseEvent { dx, dy, ..Default::default() };
        let btn = |f| MouseEvent { button_flags: f, ..Default::default() };
        c.on_mouse(mv(3, 4), T0 + 1000);
        c.on_mouse(mv(3, 4), T0 + 1050);
        c.on_mouse(btn(LDOWN), T0 + 2000);
        c.on_mouse(btn(LUP), T0 + 2080);
        c.on_mouse(btn(LDOWN), T0 + 2200); // ダブルクリック
        c.on_mouse(mv(6, 8), T0 + 2250); // ドラッグ
        c.on_mouse(btn(LUP), T0 + 2300);
        c.on_mouse(MouseEvent { button_flags: WHEEL, button_data: -240, ..Default::default() }, T0 + 4000);
        let r = c.tick(T0 + BUCKET_MS).unwrap();
        assert_eq!(r.mouse_dist, 20.0);
        assert_eq!(r.mouse_move_ms, 50);
        assert_eq!(r.click_l, 2);
        assert_eq!(r.dblclick, 1);
        assert_eq!(r.drag_dist, 10.0);
        assert_eq!(r.wheel_v, 2.0);
    }

    #[test]
    fn activity_idle_and_hand_switches() {
        let mut c = col();
        c.tick(T0);
        key(&mut c, 0x41, T0 + 500);
        c.on_mouse(MouseEvent { dx: 1, ..Default::default() }, T0 + 1500);
        key(&mut c, 0x41, T0 + 6500);
        let r = c.tick(T0 + BUCKET_MS).unwrap();
        assert_eq!(r.active_secs, 3);
        assert_eq!(r.max_idle_ms, 5000);
        assert_eq!(r.hand_switches, 2);
    }

    #[test]
    fn gap_after_sleep_discards_buckets() {
        let mut c = col();
        c.tick(T0);
        c.tick(T0 + BUCKET_MS); // T0 バケットは保存
        // スリープして 1 時間後に復帰
        assert!(c.tick(T0 + BUCKET_MS + 3_600_000).is_none());
        // 復帰直後の途中バケットも捨てる
        assert!(c.tick(T0 + 2 * BUCKET_MS + 3_600_000).is_none());
        assert!(c.tick(T0 + 3 * BUCKET_MS + 3_600_000).is_some());
    }
}
