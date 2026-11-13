# Configuration

Worminal reads configuration at run time. Nothing is compiled in, and every process resolves it itself,
including headless remote daemons.

## Files

- `$XDG_CONFIG_HOME/worminal/worminal.yaml` (default `~/.config/worminal/worminal.yaml`), or the file named by
  `WORMINAL_CONFIG` (it must exist).
- An existing Alacritty config, found in Alacritty's own search order (`alacritty.toml`, legacy `alacritty.yml`/
  `.yaml`, with `import`), or the file named by `WORMINAL_ALACRITTY_CONFIG`.

`worminal.yaml` is layered over the Alacritty config key by key, so a setup that already works keeps working and
`worminal.yaml` only needs the differences. It accepts every supported Alacritty key in YAML form, plus:

```yaml
worminal:
  update_notifications: true   # desktop notification when a window update is postponed, pending or failed
  group_tabs_by_title: true    # when a tab's title changes, the daemon moves it beside a tab with the same title
```

Supported Alacritty keys: `font` (`size`, `normal.family`, `offset.y`, `glyph_offset.y`), `colors` (`primary`,
`normal`, `bright`, `dim`, `cursor`, `indexed_colors`, `draw_bold_text_with_bright_colors`) and `scrolling`
(`history`, `multiplier`). Colors are `#rrggbb` or `0xrrggbb`; an unquoted YAML `0xrrggbb` also works.
Built-in defaults come from `assets/palette.json` and `assets/font.yaml`.

## Sync between devices

`worminald config sync [--prefer local|remote] SSH_ENDPOINT` merges `worminal.yaml` with a peer's. Views run it
automatically, once per host, right after `upgrade-remote` (the outcome goes to the daemon log).

- Each node remembers, per peer, the last agreed copy (`$XDG_STATE_HOME/worminal/config-sync/`); the merge is
  three-way per dotted key. A key changed on one side takes that side; the same change on both agrees; deleting a
  key propagates unless the other side changed it.
- Different changes to one key are conflicts: both sides keep their value and the report lists the keys until the
  values meet or `--prefer` picks a side. Nothing is overwritten silently.
- The initiator decides; the peer applies the result only if its file is unchanged since it reported it, validates
  it, and replaces it atomically. Local changes happen only after the peer accepted. Rewritten files lose comments.
- Only `worminal.yaml` syncs; Alacritty files stay per device. No background traffic: sync happens on connect.

## Behavior

- Invalid configuration makes `worminal` and `worminald` report to stderr and run with defaults; they never refuse
  to start. `worminald config` validates strictly (exit status 1 with the reason), `--paths` shows the files in use,
  `--json` prints the resolved settings and `--json --defaults` the built-in values.
- Appearance applies to windows started afterwards. Scrollback capacity is fixed when a process creates its first
  terminal, so changing `scrolling.history` needs a daemon restart (shells are preserved by the usual handoff).
- `src/config` is the only resolver. Python tooling (explorer font, the historical C reference build) calls
  `worminald config --json` instead of reimplementing the rules.
