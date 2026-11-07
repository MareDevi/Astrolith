# Writing native input acceptance

- Tested revision: `11d89d0`, local `dev`; gpui-pre dev build.
- Environment: unlocked Hyprland/Wayland with fcitx5/Rime. A disposable Linux uinput keyboard sent events only while the isolated window was focused. The test matched the window PID to the temporary fixture path before each event. The keyboard was removed when the test ended.
- Fixture: `history-single-paragraph-102420`, 102,420 bytes. The harness copies it to a disposable project. No user document was edited.

Launch:

```sh
node scripts/editor-perf.mjs run --fixture history-single-paragraph-102420 --backend pre --scenario idle --duration 36000 --timeout 60 --test-id editor-followup-ime-rime-20261107
```

Use the fixture path recorded in `command.txt` and keep the test window focused:

1. Select Rime in the focused Writing window. In this session `fcitx5-remote -o` followed by `fcitx5-remote -s rime` selected it. Verify it by observing actual candidates; the controller state alone did not establish composition.
2. Enter `ni`. Verify inline preedit, visible caret and candidate `你` beneath the composing range. Press `Ctrl+S` before confirming it. The bytes are still identical to the original file.
3. Press Space, then `Ctrl+S`. Compare the file to the original with only `你` prepended (3 UTF-8 bytes).
4. Enter `hao`. Verify a second candidate window. Press Escape, then `Ctrl+S`. Compare the file to the saved `你` revision; it is unchanged.
5. Press `Ctrl+Z`, then `Ctrl+S`. The file is exactly the original fixture.
6. Switch to the English keyboard, type `ABC ` and save. Compare the file to the original with only that prefix prepended.

`keyboard-check.json` records the byte comparisons. The screenshots show the two candidate windows and the visible input caret. Earlier injection attempts remained in plain keyboard mode and were excluded from IME acceptance.
