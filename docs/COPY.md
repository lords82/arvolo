# Arvolo — writing the words

The words in the CLI and the GUI follow one rule, and the rest of this file is
that rule applied to particular places.

> **Say what happened. Say what they can do about it. Put the reason somewhere
> else.**

## Why there is a rule at all

The copy this replaced was not careless — it was careful in the wrong direction.
Almost every string said the true and useful thing in its first three words and
then explained itself:

```
Blocked 'alice' — their offers are dropped on arrival, silently.
I can't reach the daemon. Transfers already running carry on, but this window
  can't see them.
Linking is done from both ends: … It is a delicate operation — what crosses over
  is your secret identity, not a mere invitation.
```

Each explanation is correct, and each one is read once with interest and then a
hundred more times by somebody who already knows it and is in a hurry. That is
the cost: not that the sentence is long, but that the part worth reading is
buried in front of a part that has stopped being worth reading.

The explanations are not deleted. They move.

## The three slots

| slot | what it holds | GUI | CLI |
|---|---|---|---|
| **1 · Outcome** | what happened, or what this is | the label, title, first line | the line on stderr |
| **2 · Action** | what the user can do now | a button, or one line | one line, command indented four spaces |
| **3 · Reason** | why, how it works, what it costs | the hint line underneath | `-v` (`vprintln!`) |

Which slot a sentence belongs to has one test:

> **If I delete this, is the user stuck?**

Yes → slot 1 or 2. No → slot 3.

**Slot 2 is never demoted.** An error that says `P2P is off` and stops has told
somebody they cannot proceed and not how to. They will not find the way out
behind a `-v` they do not know exists.

Slot 3 is never mandatory. If a reason will not fit in one sentence, it is not a
hint — it is documentation, and it belongs in `MANUAL.md` with a pointer to it.

## Length

Caps are in characters and measured against English. `gui/src/test/copy.test.ts`
enforces them; `gui/src/test/copy-rules.ts` maps key names to categories.

| category | cap | shape |
|---|---|---|
| label, badge, column heading | 24 | a noun, no full stop |
| button, menu item, palette entry | 28 | **a verb**, no full stop |
| section / sheet / dialog / empty-state title | 40 | a noun phrase, no full stop |
| toast title | 40 | the outcome |
| status line, row metadata | 48 | a fragment, lowercase |
| error, first line | 60 | what failed |
| **hint, secondary line, blurb** | **120, one sentence** | a full sentence |
| confirmation body | 140, two sentences at most | the consequence |

CLI:

| | |
|---|---|
| outcome / error line | one line ≤ 100, no `\n` inside it |
| action line | one command, on its own line, indented four spaces |
| first line of a `Command` doc comment | **60** — it is the column in the command list |
| body of a `Command` doc comment | three lines |
| help for one `#[arg]` | one line ≤ 80; two only to state a default or a conflict |
| comment on a `config.toml` key | two lines, then a link into `MANUAL.md` |

**No more than three consecutive lines on stderr** on a non-verbose path (lists of
data and progress bars excepted). The fourth line is a `vprintln!`.

### The floor

Six strings have a *minimum* as well as a maximum, listed in `FLOOR` in
`copy-rules.ts`. They are the last thing somebody reads before an irreversible
act — handing over their identity, downloading from a key nobody has checked.
`Careful.` satisfies every cap above and warns nobody, so each one carries a note
saying what it must still contain. Shortening these is the one way this rule can
do damage.

## Voice

**The app has no first person.** It is not reaching, trying, or recognising
anything.

```
I can't reach the daemon.               →  Daemon unreachable
I don't recognise this shape. I'll try  →  Unfamiliar format — trying it anyway
  it anyway — the daemon is fussier
  than I am
```

The exception is where the first person is **the user's**: a button or a checkbox
is worded as what the user is saying by clicking it, so *"I compared the
fingerprint"* on a confirmation is right. `USER_VOICE` in `copy.test.ts` lists
these, each with the reason.

Also out:

- adverbs of commentary — `silently`, `simply`, `just`, `merely`
- hedges — `it seems`, `appears to be`
- appraisals — `It is a delicate operation`

**A button is a verb.** `Trust`, not `Mark as trusted: download automatically`.
Whatever the label was explaining goes on the line under it.

**One em dash per string, in the hint only, and only parenthetically.** Never in
a primary line, and never to weld two sentences into one. `hint + " — " + raw`
built in code is not exempt: it is the same sentence, assembled later.

**Cite a command only where it can be run**, on its own line, one per message —
and only if it exists. `every_cited_command_exists` in `cli/src/args.rs` checks
that last part, because five messages had drifted onto flags that had been
renamed years earlier.

## One fact, one place

A fact appears **once per screen** and **once per concept**. Where it goes is
decided by one question: *where is the user about to make the choice that depends
on it?* It goes there, and nowhere else.

Before this rule: "the relay never sees your files in the clear" appeared five
times, "compare the fingerprint out loud" nine, and "whoever types this code
becomes you" three — twice on the same screen, sixteen lines apart.

Not every repetition is one, though. Two screens can need the same fact at two
different moments, and then both keep it. The test is the moment, not the words.

## What this rule does not touch

- **`after_help` examples in `args.rs`.** Two aligned columns, one use case per
  line, no prose. They are long as a *block*, which is not the thing being
  measured.
- **stdout is the artefact, stderr is the narration.** A rewrite that tidies a
  channel breaks pipes and the integration tests that read them.
- **The status glyphs** `✓ ✗ ⚠ ⏳ ⏸ ↓ ·`, lowercase for errors, capitalised for
  successes. Grammar, not prose.
- **`cli/src/notify.rs` is in Italian on purpose** — those notifications sit in a
  desktop notification centre next to the GUI's own, which are Italian, and often
  reach somebody who never opened a terminal. The reasoning is in its header.
- **Values read aloud** — fingerprints, ids, codes. Data, not copy: never
  shortened, never truncated.
- **The ~390 micro-labels** already at a median of 17 characters. They were
  always the best part of the dictionary.

## Translating

`en.ts` is the source dictionary and `Dict` is derived from it, so the four files
move together whenever the *set* of keys changes.

Italian is not a translation. `it.ts` says so in its own header: most of it is the
original text of the GUI, written before the app had languages at all. It is
rewritten under these rules directly, not translated back from English.

French and German follow the rewritten English. German goes last: its compounds
are the longest, so it is what reveals a cap that was set too tight while there
is still time to widen it. `LANG_SLACK` in `copy-rules.ts` gives each language
its own room — the cap is written for English and the others are not English.

Two keys with the same English text are **not** merged by default. Merge only
when all four languages already agree: if Italian already tells them apart,
somebody has already decided they are different things. `Send` is four keys and
`System` is two, for exactly that reason.
