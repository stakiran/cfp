# cfp — context fingerprint collector

キーボードとマウスの入力を10秒単位で集計し、SQLite に記録する Windows 常駐ツール。
キーコード、文字、カーソル座標、アプリ名は一切保存しない。

## ビルド

```
cargo build --release
```

`target\release\cfp.exe` ができる。コンソールは出ず、タスクトレイにアイコンが出る。
トレイアイコンをクリックして「終了」で止まる。多重起動はしない。

## 自動起動

`Win + R` → `shell:startup` で開いたフォルダに `cfp.exe` のショートカットを置く。

## データ

`cfp.exe` と同じフォルダの `cfp.db` の `buckets` テーブル。1行 = 10秒。
WAL モードなので、記録中でも Python などから読める。

| 列 | 内容 |
|---|---|
| t | バケット開始時刻（Unix秒, UTC） |
| key_char | 英数字・記号キー |
| key_space / key_bs / key_enter / key_tab / key_esc | 各キー（key_bs は Backspace と Delete） |
| key_nav | 矢印, Home/End, PgUp/PgDn |
| key_shortcut | Ctrl / Alt / Win と同時に押したキー |
| key_ime | 変換, 無変換, 半角/全角, かな, 英数 など |
| iki_lt100 〜 iki_ge1000 | 打鍵間隔の4段階の件数（修飾キー除く） |
| mouse_dist | マウス移動量（Raw Input の相対単位） |
| mouse_move_ms | マウスが動いていた時間 |
| click_l / click_r / click_m | クリック数 |
| dblclick | ダブルクリック数 |
| drag_dist | ボタンを押したままの移動量 |
| wheel_v / wheel_h | ホイールのノッチ数（絶対値） |
| active_secs | 入力があった秒の数（0〜10） |
| max_idle_ms | バケット内の最長無入力時間 |
| hand_switches | キーボードとマウスの持ち替え回数 |

## 仕様メモ

- キーの自動リピートはどのカウントにも含めない。
- 起動直後・終了直前・スリープ前後の途中バケットは保存しない。
  すべての行は「境界から境界まで連続して観測できた10秒」になっている。
- 入力ゼロのバケットも保存する（分析時に `active_secs > 0` で絞る）。
- リモートデスクトップやペンタブレットの絶対座標入力は移動量に数えない（クリックは数える）。
- 管理者権限で動くウィンドウ（タスクマネージャー等）が前面にある間の入力は、
  cfp を管理者で実行しない限り届かない。
- ロック画面や UAC 画面の入力は届かない。
