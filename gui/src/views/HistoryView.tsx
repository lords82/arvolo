// The log of what has finished. Read-only by construction: anything still
// actionable is a live row on the board, not a line here.
//
// Grouped by real calendar day using the engine's own timestamp rather than when
// this window happened to see it — otherwise every restart would file last
// month's transfers under "Oggi".

import { useMemo, useState } from "react";
import { fire, useStore } from "../store";
import { fmtBytes } from "../format";
import { locale, t as translate, useLang, useT } from "../i18n";
import { Icon } from "../ui/Icons";
import { Button, Empty, Segmented, TextInput } from "../ui/Primitives";
import { ExtChip } from "../ui/Bits";
import { Confirm } from "../ui/Sheet";
import type { HistoryDto } from "../types";

type Filter = "all" | "send" | "recv";

/** Built per call rather than once at import: the weekday and month names, and
 *  whether the clock is 12- or 24-hour, both come from the active language. A
 *  module-level formatter would keep the launch language for ever. */
function dayFmt(): Intl.DateTimeFormat {
  return new Intl.DateTimeFormat(locale(), {
    weekday: "long",
    day: "numeric",
    month: "long",
  });
}
function timeFmt(): Intl.DateTimeFormat {
  return new Intl.DateTimeFormat(locale(), {
    hour: "2-digit",
    minute: "2-digit",
  });
}

function dayKey(unixSecs: number): string {
  const d = new Date(unixSecs * 1000);
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

function dayLabel(unixSecs: number): string {
  const d = new Date(unixSecs * 1000);
  const today = new Date();
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  if (dayKey(unixSecs) === dayKey(Math.floor(today.getTime() / 1000)))
    return translate("history.today");
  if (dayKey(unixSecs) === dayKey(Math.floor(yesterday.getTime() / 1000)))
    return translate("history.yesterday");
  return dayFmt().format(d);
}

/** Split a daemon status into a word and its tone. `failed: …` carries its
 *  reason after the colon and that reason is the useful half. */
function outcome(status: string): { text: string; tone: string; detail?: string } {
  if (status === "completed")
    return { text: translate("history.completed"), tone: "ok" };
  if (status === "cancelled")
    return { text: translate("history.cancelled"), tone: "mut" };
  if (status === "deposited")
    return { text: translate("history.deposited"), tone: "in" };
  const [head, ...rest] = status.split(":");
  if (head === "failed")
    return {
      text: translate("history.failed"),
      tone: "bad",
      detail: rest.join(":").trim(),
    };
  // Anything the engine invents that this build has not learned yet: name the
  // fact, and keep the raw string as the detail rather than putting an
  // untranslated wire token in the status slot.
  return {
    text: translate("history.unknownOutcome"),
    tone: "mut",
    detail: status,
  };
}

function Row({ h }: { h: HistoryDto }) {
  const t = useT();
  const peerLabel = useStore((s) => s.peerLabel);
  const o = outcome(h.status);
  const out = h.direction === "send";
  return (
    <div className={`row dir-${out ? "out" : "in"} is-done`}>
      <ExtChip name={h.name} />
      <div className="row-main">
        <div className="row-name truncate" title={h.name}>
          {h.name}
        </div>
        <div className="row-meta">
          <span className={`tone-${o.tone}`} style={{ fontWeight: 600 }}>
            {o.text}
          </span>
          <span className="sep" />
          <span className="truncate">
            {out ? t("common.to") : t("common.from")} {peerLabel(h.peer)}
          </span>
          <span className="sep" />
          <span className="tnum">{fmtBytes(h.total_size)}</span>
          {o.detail && (
            <>
              <span className="sep" />
              <span className="truncate" title={o.detail}>
                {o.detail}
              </span>
            </>
          )}
        </div>
      </div>
      <span className="t-xs t-mut tnum">
        {timeFmt().format(new Date(h.created * 1000))}
      </span>
    </div>
  );
}

export function HistoryView() {
  const t = useT();
  // Not for a string on this screen — for the day headings, which `dayLabel`
  // builds through `Intl` and the memo below caches.
  const lang = useLang();
  const history = useStore((s) => s.history);
  const loading = useStore((s) => s.historyLoading);
  const error = useStore((s) => s.historyError);
  const reload = useStore((s) => s.loadHistory);
  const clear = useStore((s) => s.clearHistory);
  const peerLabel = useStore((s) => s.peerLabel);

  const [filter, setFilter] = useState<Filter>("all");
  const [q, setQ] = useState("");
  const [confirmClear, setConfirmClear] = useState(false);

  const groups = useMemo(() => {
    const needle = q.trim().toLowerCase();
    const rows = history.filter((h) => {
      if (filter !== "all" && h.direction !== filter) return false;
      if (!needle) return true;
      return (
        h.name.toLowerCase().includes(needle) ||
        peerLabel(h.peer).toLowerCase().includes(needle)
      );
    });
    const map = new Map<string, { label: string; when: number; items: HistoryDto[] }>();
    for (const h of rows) {
      const k = dayKey(h.created);
      const g = map.get(k);
      if (g) g.items.push(h);
      else map.set(k, { label: dayLabel(h.created), when: h.created, items: [h] });
    }
    return Array.from(map.values()).sort((a, b) => b.when - a.when);
  }, [history, filter, q, peerLabel, lang]);

  return (
    <div className="stack">
      <div className="hstack wrap">
        <Segmented
          label={t("history.filterLabel")}
          value={filter}
          onChange={setFilter}
          options={[
            { value: "all", label: t("history.filterAll") },
            { value: "send", label: t("history.filterSent") },
            { value: "recv", label: t("history.filterReceived") },
          ]}
        />
        <div className="grow" style={{ maxWidth: 280 }}>
          <TextInput
            value={q}
            onChange={(e) => setQ(e.currentTarget.value)}
            placeholder={t("history.searchPlaceholder")}
            aria-label={t("history.searchLabel")}
          />
        </div>
        <div className="spacer grow" />
        <Button size="sm" onClick={() => fire(reload())} busy={loading}>
          <Icon.Refresh size={13} /> {t("common.refresh")}
        </Button>
        <Button
          size="sm"
          variant="danger"
          disabled={!history.length}
          onClick={() => setConfirmClear(true)}
        >
          <Icon.Trash size={13} /> {t("history.clear")}
        </Button>
      </div>

      {error && (
        <div className="card card-pad t-sm" style={{ borderColor: "var(--red)" }}>
          {error}
        </div>
      )}

      {groups.length === 0 ? (
        <div className="card">
          <Empty
            icon={<Icon.History size={22} />}
            title={
              history.length
                ? t("history.emptyNoMatch")
                : t("history.emptyNothing")
            }
          >
            {history.length
              ? t("history.emptyNoMatchBody")
              : t("history.emptyNothingBody")}
          </Empty>
        </div>
      ) : (
        groups.map((g) => (
          <div key={g.label + g.when} className="section">
            <div className="section-head">
              <span className="t-label">{g.label}</span>
              <span className="t-xs t-mut tnum">{g.items.length}</span>
            </div>
            <div className="card rows">
              {g.items.map((h) => (
                <Row key={h.id} h={h} />
              ))}
            </div>
          </div>
        ))
      )}

      <Confirm
        open={confirmClear}
        title={t("history.confirmClearTitle")}
        body={t("history.confirmClearBody")}
        confirmLabel={t("history.clear")}
        danger
        onCancel={() => setConfirmClear(false)}
        onConfirm={() => {
          setConfirmClear(false);
          fire(clear());
        }}
      />
    </div>
  );
}
