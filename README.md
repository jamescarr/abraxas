# ABRAXAS

A 1999 BBS hacker game that dreams its own levels.

## Writing a story pack

A story pack is the starting story. You write the first levels, the cast and the voice; the AI director adapts your story to the player's choices and keeps it going. Without an AI, the offline generator draws from your pack's word bank (`pool.toml`), so a pack is always playable.

Copy `packs/_template/` to begin: every key in it is commented, and it is the second reference to this section. The built-in pack, `packs/rollover/`, is a full example.

```
abraxas --pack path/to/my-pack          play a pack from a directory
abraxas pack check [DIR]                validate a pack (the built-in pack when DIR is omitted)
```

### Layout

```
my-pack/
  pack.toml         identity, voice, motifs, acts, the endless tail      (required)
  canon.md          the setting, second person, at most 1500 words       (required)
  cast.toml         characters                                           (required)
  anchors/NN-slug.toml  authored levels, one per file
  pool.toml         word bank for the offline generator                  (optional)
  art/*.txt         pipe-coded art                                       (optional)
```

A pack can be as small as `pack.toml`, `canon.md`, `cast.toml` and one anchor.

### pack.toml

```toml
id = "rollover"                  # ^[a-z0-9-]{1,32}$
title = "ABRAXAS: The Rollover"  # not empty
author = "James Carr"
version = "1"
style = """at most 120 words of voice rules sent to the AI with every call"""
motifs = ["wet carpet", "a payphone ringing"]    # at least 8; two are drawn per level
room_kinds = ["a school district board"]         # at least 8; one is drawn per level
start_clock = "23:00:00"         # HH:MM:SS, the in-world time at level 0
minutes_per_level = 4            # at least 1

[[act]]                          # one or more, contiguous from depth 0
id = "nigredo"                   # ^[a-z0-9_]{1,24}$, unique
name = "NIGREDO"
depths = [0, 3]                  # inclusive; no gap, no overlap, from <= to
palette = "nigredo"              # nigredo | albedo | citrinitas | rubedo | prism
goal = "at most 160 characters"
direction = """at most 120 words for the AI"""
default_vision = "fire"          # tunnel | plasma | mandala | fire | ouroboros | starfield | mirror

[endless]                        # what happens after the last act
enabled = true                   # false: the story ends there
name = "THE UNDERBOARD"          # name, goal and direction are required when enabled
palette = "prism"
goal = "..."
direction = """..."""
default_vision = "starfield"
```

### cast.toml

```toml
[[character]]
id = "philemon"                  # ^[a-z0-9_]{1,24}$, unique
handle = "PHILEMON"              # "{shadow}" only for the shadow archetype (at most one)
archetype = "wise_old_man"       # persona | shadow | syzygy | anima | animus | wise_old_man
                                 # | great_mother | trickster | self | child | hero
from_depth = 0                   # first depth where the character exists
focusable = true                 # may be picked as a level's focus
description = "at most 60 words"
voice = "at most 40 words"
art = "philemon.txt"             # optional; must exist in art/
fallback = ["...", "...", "..."] # chat lines used without an AI, at least 3

[[character.eliza]]              # keyword rules for the no-AI chat
keywords = ["wren", "sister"]    # first group with a matching keyword answers
replies = ["...", "..."]         # must not be empty
```

### Anchors

An anchor is an authored level at a fixed depth, in `anchors/NN-slug.toml`. Depths are unique and inside the acts (beyond the last act only when the endless tail is enabled). Every key except `depth` is optional.

```toml
depth = 3
ai = "adapt"                     # adapt (default) | off: never call the AI for this level
title = "THE BLACK SUN"          # at most 40
bbs_name = "SOL NIGER"           # at most 24
tagline = "..."                  # at most 60
sysop = "shadow"                 # a cast id, or a handle ^[A-Za-z0-9_-]{2,16}$
danger = 2                       # 1..5; default is 1 + depth/4, at most 5
focus = "shadow"                 # a cast id
vision = "fire"
programs = ["GHOST.COM"]         # DECRYPT.COM SNIFFER.EXE BUFFER.EXE GHOST.COM
                                 # ICEPICK.EXE MIRROR.EXE MANDALA.SYS
password = "RAVENS"              # A-Z, as long as the WORDLOCK word for `danger`
level_card = """..."""           # at most 320
required = """facts the AI must keep"""
interlude = """..."""
beat = """..."""                 # at most 700
captions = ["...", "...", "..."] # at most 3, each at most 48
summary = "..."                  # at most 200
words = ["RAVENS", "CINDER"]     # extra password candidates, each the same length

[host]
name = "ODS LOWTIDE EXCHANGE"    # at most 40
os = "unix"                      # unix | vax | mainframe | dos
banner = "..."                   # at most 160
objective_kind = "retrieve"      # retrieve | plant | crash
objective = "..."                # at most 100
target_file = "WREN-01.LOG"      # 8.3 name
target_daemon = "pleroma"        # crash objectives only; lowercase process name
objective_ice = "shadow"         # breach | black | shadow
nodes = 5                        # 3..8
node_names = ["..."]             # each at most 24
ice_names = ["..."]              # each at most 20

[[post]]                         # from <= 16, subject <= 40, body <= 600
from = "..."
subject = "..."
body = """..."""

[[text_file]]                    # BBS file library; name is 8.3, description <= 40, body <= 900
name = "HOWTO.TXT"
description = "..."
body = """..."""

[[host_file]]                    # files found on the host; same fields
name = "SHIFTLOG.TXT"
description = "..."
body = """..."""

[choice]                         # 2 or 3 options; prompt <= 160, option text <= 70
prompt = "..."
[[choice.option]]
text = "..."
stance = "integrate"             # integrate | repress | neutral
flags = ["answered_shadow"]      # at most 2, ^[a-z][a-z0-9_]{0,31}$

[[exit]]                         # all three, once each, text <= 60
kind = "calm"                    # calm | deep | mirror
text = "..."

[[ending]]                       # at an anchor that ends the story or the arc
id = "individuation"
title = "INDIVIDUATION"
when_stance = "integrate"
min_integration = 3              # optional
max_integration = 10             # optional
vision = "mandala"               # optional, default mandala
text = """..."""
```

Endings are tried in file order; the first whose stance matches the player's choice and whose integration bounds hold wins. If an anchor has endings, every option of its choice needs at least one ending for its stance.

How an anchor mixes with the AI:

| Class | Fields | Rule |
|---|---|---|
| Locked | `title`, `bbs_name`, `tagline`, `sysop`, `danger`, `focus`, `vision`, `programs`, `password`, every `[host]` key, `choice`, `exit`, `ending` | When present, your value wins and any AI value is ignored. |
| Adapted | `level_card`, `beat`, `interlude`, `captions`, `summary` | With `ai = "adapt"` the AI is told to keep every fact and reword to fit the story so far, together with `required`. A valid AI result is used, otherwise your text. |
| Appended | `post`, `text_file`, `host_file`, `words` | Your entries always come first; the AI or the pool fills the remaining slots (4 posts, 2 text files, 3 host files). |
| Generated | every level without an anchor | Fully AI-generated, or built from the pool offline. |
| Continued | depths beyond the last act, when `endless.enabled` | AI levels forever, using the `[endless]` goal and direction plus the story so far. |
| Finished | the last act's final depth, when `endless.enabled = false` | After that level's choice a matching ending plays, otherwise the generic `END OF LINE`. The run is marked complete. |

With `ai = "off"` the AI is never called for that level and missing fields come from the offline generator.

### pool.toml and inheritance

`pool.toml` is the offline generator's word bank. Any list that is missing or empty is inherited from the built-in pool, list by list, so you only write what you want to change.

| Key | Contents |
|---|---|
| `titles`, `bbs_names`, `taglines` | at most 40, 24 and 60 characters |
| `level_cards` | at most 320; may use `{motif}` |
| `sysops` | handles `^[A-Za-z0-9_-]{2,16}$` |
| `host_names`, `node_names`, `ice_names` | at most 40, 24 and 20 |
| `daemons` | lowercase process names |
| `interludes` | may use `{exit}` |
| `beats`, `captions` | at most 700 and 48 |
| `words_5` ... `words_8` | uppercase words matching `^[A-Z]{N}$` |
| `eliza`, `eliza_fallback` | `{ keywords, replies }` groups and fallback lines for the generic NPC sysop chat |

`[[post]]`, `[[text_file]]`, `[[host_file]]` and `[[exit]]` use the same fields as in anchors; a `[[choice]]` with `[[choice.option]]` must offer exactly one option per stance, and the exits must include every kind.

### Placeholders

Authored text can contain `{handle}` (the player), `{shadow}` (the player's Shadow handle) and `{real_name}`. Pool level cards may also use `{motif}` and pool interludes `{exit}`. Write a literal brace as `{{` or `}}`. Any other `{...}` is an error.

### Pipe codes and art

Authored text and art use pipe codes for colour: `|00` to `|15` set the foreground to a DOS colour, `|16` to `|23` set the background to DOS colours 0 to 7. The colour lasts to the end of the text block. A `|` not followed by two digits in range is literal.

| Code | Colour | Code | Colour |
|---|---|---|---|
| `\|00` | black | `\|08` | dark gray |
| `\|01` | blue | `\|09` | light blue |
| `\|02` | green | `\|10` | light green |
| `\|03` | cyan | `\|11` | light cyan |
| `\|04` | red | `\|12` | light red |
| `\|05` | magenta | `\|13` | light magenta |
| `\|06` | brown | `\|14` | yellow |
| `\|07` | light gray | `\|15` | white |

Backgrounds: `|16` black, `|17` blue, `|18` green, `|19` cyan, `|20` red, `|21` magenta, `|22` brown, `|23` light gray.

Everything the game draws stays in the CP437 character set: ASCII, box drawing, blocks, shades and the smiley glyphs. Curly quotes, long dashes, the ellipsis character, accents outside CP437 and emoji are errors. Art lives in `art/*.txt`, at most 36 columns by 16 rows after the pipe codes are removed; `abraxas-logo.txt` (the login screen) may be 78 by 8. A character's `art` names a file in `art/`.

File names on the board and on hosts follow DOS 8.3: `^[A-Z0-9_-]{1,8}\.[A-Z0-9]{1,3}$`, for example `WREN-01.LOG`.

### Checking a pack

`abraxas pack check [DIR]` validates the pack in `DIR`, or the built-in pack. Every problem is listed, one per line, as `{file}: {field}: {message}`, and the exit code is 1:

```
anchors/03-black-sun.toml: depth: depth 0 already used by anchors/00-the-lobby.toml
anchors/15-the-center.toml: ending: option 2 (repress) has no ending
```

On success it prints `pack {id}: OK ({acts} acts, {anchors} anchors, {characters} characters, endless {on|off})` and exits 0. The checks cover the identity and clock in `pack.toml`; acts contiguous from depth 0; cast ids, shadow handle, art files and ELIZA lists; unique anchor depths, sysop and focus ids, 2 to 3 choice options, exactly three exits, endings for every option, password length for the danger class, and 8.3 names; every length limit above; CP437 text and art; art size; pool lists and placeholders.
