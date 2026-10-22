// One icon set, drawn rather than imported.
//
// Every glyph is a 16×16 stroked path on `currentColor`, so an icon inherits the
// colour of whatever it sits in and needs no dark-mode variant. They are inline
// SVG for the same reason the app has no CDN: a Tauri window with a strict CSP
// cannot fetch an icon font, and shipping one as a binary would put 200 glyphs in
// the bundle to use thirty.
//
// The geometry is deliberately plain — 1.6px strokes at 16, round joins, 2px
// inset — because these sit next to 12px labels and any more detail turns to
// mud.
//
// The stroke is not a constant. A 16-unit path drawn at 24px scales its 1.6
// stroke up to 2.4 *on screen*, which is not what the system's own glyphs do:
// SF Symbols get heavier in absolute terms as they grow but lighter relative to
// their size, so a 24pt symbol next to a 16pt one reads as the same weight.
// `weight()` is that curve — 1.6px on screen at 16, 2.1 at 24, 2.35 at 28 —
// expressed in viewBox units, so a 16px inline glyph is unchanged and a tab
// glyph is a little lighter than it was.

interface Props {
  size?: number;
  className?: string;
  /** Decorative by default; pass a label when the icon *is* the button. */
  label?: string;
}

/** Stroke width in viewBox units for a glyph drawn `size` px wide. */
function weight(size: number): number {
  return Math.round((16 / size) * (0.6 + size / 16) * 100) / 100;
}

function Svg({
  size = 16,
  className,
  label,
  children,
}: Props & { children: React.ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={weight(size)}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      focusable="false"
    >
      {children}
    </svg>
  );
}

export const Icon = {
  Send: (p: Props) => (
    <Svg {...p}>
      <path d="M3.5 12.5 12.5 3.5" />
      <path d="M6 3.5h6.5V10" />
    </Svg>
  ),
  Receive: (p: Props) => (
    <Svg {...p}>
      <path d="M12.5 3.5 3.5 12.5" />
      <path d="M10 12.5H3.5V6" />
    </Svg>
  ),
  Transfers: (p: Props) => (
    <Svg {...p}>
      <path d="M2.5 5.5h9M9 3l2.5 2.5L9 8" />
      <path d="M13.5 10.5h-9M7 8l-2.5 2.5L7 13" />
    </Svg>
  ),
  People: (p: Props) => (
    <Svg {...p}>
      <circle cx="6" cy="5.5" r="2.5" />
      <path d="M1.8 13.5c0-2.3 1.9-3.8 4.2-3.8s4.2 1.5 4.2 3.8" />
      <path d="M11 3.4a2.4 2.4 0 0 1 0 4.4M12.2 9.9c1.3.5 2.1 1.7 2.1 3.6" />
    </Svg>
  ),
  Link: (p: Props) => (
    <Svg {...p}>
      <path d="M6.6 9.4a2.6 2.6 0 0 0 3.7 0l2-2a2.6 2.6 0 0 0-3.7-3.7l-.9.9" />
      <path d="M9.4 6.6a2.6 2.6 0 0 0-3.7 0l-2 2a2.6 2.6 0 0 0 3.7 3.7l.9-.9" />
    </Svg>
  ),
  History: (p: Props) => (
    <Svg {...p}>
      <circle cx="8" cy="8" r="6" />
      <path d="M8 4.6V8l2.4 1.5" />
    </Svg>
  ),
  Devices: (p: Props) => (
    <Svg {...p}>
      <rect x="1.6" y="3" width="8.6" height="6.4" rx="1.2" />
      <path d="M4 12.4h4.4" />
      <rect x="11.4" y="6.6" width="3" height="6.8" rx="1" />
    </Svg>
  ),
  /* Una ruota dentata, non un sole. Era un mozzo con otto raggi lunghi, cioè
     lo stesso disegno di `Sun` a un paio di decimi di distanza: nella barra
     in alto, che è la porta di Impostazioni, si leggeva come la luminosità.
     La corona chiusa è ciò che fa la differenza — un sole non ce l'ha — e i
     denti partono da lì invece che dal centro. */
  Settings: (p: Props) => (
    <Svg {...p}>
      <circle cx="8" cy="8" r="5.3" />
      <circle cx="8" cy="8" r="2.2" />
      <path d="M8 1.5v1.2M8 13.3v1.2M14.5 8h-1.2M2.7 8H1.5M12.6 3.4l-.85.85M4.25 11.75l-.85.85M12.6 12.6l-.85-.85M4.25 4.25 3.4 3.4" />
    </Svg>
  ),
  Search: (p: Props) => (
    <Svg {...p}>
      <circle cx="7" cy="7" r="4.4" />
      <path d="m10.4 10.4 3 3" />
    </Svg>
  ),
  Plus: (p: Props) => (
    <Svg {...p}>
      <path d="M8 3.2v9.6M3.2 8h9.6" />
    </Svg>
  ),
  Close: (p: Props) => (
    <Svg {...p}>
      <path d="m4 4 8 8M12 4l-8 8" />
    </Svg>
  ),
  Check: (p: Props) => (
    <Svg {...p}>
      <path d="m3.2 8.4 3.2 3.2 6.4-7.2" />
    </Svg>
  ),
  Copy: (p: Props) => (
    <Svg {...p}>
      <rect x="5.6" y="5.6" width="8" height="8" rx="1.4" />
      <path d="M10.8 3.4a1.4 1.4 0 0 0-1.4-1.4H3.8a1.4 1.4 0 0 0-1.4 1.4v5.6a1.4 1.4 0 0 0 1.4 1.4" />
    </Svg>
  ),
  Qr: (p: Props) => (
    <Svg {...p}>
      <rect x="2.2" y="2.2" width="4.4" height="4.4" rx="1" />
      <rect x="9.4" y="2.2" width="4.4" height="4.4" rx="1" />
      <rect x="2.2" y="9.4" width="4.4" height="4.4" rx="1" />
      <path d="M9.4 9.4h2v2h-2zM13.8 9.4v1M13.8 13.8h-2.4M13.8 12.2v.2" />
    </Svg>
  ),
  Pause: (p: Props) => (
    <Svg {...p}>
      <path d="M6 3.4v9.2M10 3.4v9.2" />
    </Svg>
  ),
  Play: (p: Props) => (
    <Svg {...p}>
      <path d="M4.6 3.2 12.4 8l-7.8 4.8z" />
    </Svg>
  ),
  Stop: (p: Props) => (
    <Svg {...p}>
      <circle cx="8" cy="8" r="6" />
      <path d="m5.8 5.8 4.4 4.4M10.2 5.8l-4.4 4.4" />
    </Svg>
  ),
  Trash: (p: Props) => (
    <Svg {...p}>
      <path d="M2.8 4.4h10.4M6.4 4.4V3.2a1 1 0 0 1 1-1h1.2a1 1 0 0 1 1 1v1.2" />
      <path d="M4.2 4.4l.6 8.2a1 1 0 0 0 1 .9h4.4a1 1 0 0 0 1-.9l.6-8.2" />
    </Svg>
  ),
  More: (p: Props) => (
    <Svg {...p}>
      <circle cx="3.4" cy="8" r=".9" fill="currentColor" stroke="none" />
      <circle cx="8" cy="8" r=".9" fill="currentColor" stroke="none" />
      <circle cx="12.6" cy="8" r=".9" fill="currentColor" stroke="none" />
    </Svg>
  ),
  ChevronRight: (p: Props) => (
    <Svg {...p}>
      <path d="m6 3.6 4.4 4.4L6 12.4" />
    </Svg>
  ),
  /** The way back, which on a phone is a chevron and a word. */
  ChevronLeft: (p: Props) => (
    <Svg {...p}>
      <path d="M10 3.6 5.6 8l4.4 4.4" />
    </Svg>
  ),
  ChevronDown: (p: Props) => (
    <Svg {...p}>
      <path d="M3.6 6 8 10.4 12.4 6" />
    </Svg>
  ),
  Shield: (p: Props) => (
    <Svg {...p}>
      <path d="M8 1.8 13 3.6v4.2c0 3.1-2 5.3-5 6.4-3-1.1-5-3.3-5-6.4V3.6z" />
      <path d="m5.9 7.9 1.5 1.5 2.9-3.2" />
    </Svg>
  ),
  Star: (p: Props) => (
    <Svg {...p}>
      <path d="m8 2 1.9 3.9 4.3.6-3.1 3 .7 4.3L8 11.8 4.2 13.8l.7-4.3-3.1-3 4.3-.6z" />
    </Svg>
  ),
  Ban: (p: Props) => (
    <Svg {...p}>
      <circle cx="8" cy="8" r="6" />
      <path d="m3.8 3.8 8.4 8.4" />
    </Svg>
  ),
  Folder: (p: Props) => (
    <Svg {...p}>
      <path d="M1.9 4.4a1.2 1.2 0 0 1 1.2-1.2h2.6l1.4 1.8h5a1.2 1.2 0 0 1 1.2 1.2v5.6a1.2 1.2 0 0 1-1.2 1.2H3.1a1.2 1.2 0 0 1-1.2-1.2z" />
    </Svg>
  ),
  /** Photos and videos: the mountain-and-sun every picker uses for a library. */
  Image: (p: Props) => (
    <Svg {...p}>
      <rect x="2" y="3.2" width="12" height="9.6" rx="1.6" />
      <circle cx="5.6" cy="6.4" r="1.1" />
      <path d="M2.4 11.4 6 8.2l2.3 2.1 2.4-2.6 2.9 3" />
    </Svg>
  ),
  Refresh: (p: Props) => (
    <Svg {...p}>
      <path d="M13.4 7a5.5 5.5 0 0 0-9.6-2.4L2.4 6" />
      <path d="M2.6 9a5.5 5.5 0 0 0 9.6 2.4L13.6 10" />
      <path d="M2.4 2.8V6h3.2M13.6 13.2V10h-3.2" />
    </Svg>
  ),
  Sun: (p: Props) => (
    <Svg {...p}>
      <circle cx="8" cy="8" r="2.8" />
      <path d="M8 1.4v1.4M8 13.2v1.4M14.6 8h-1.4M2.8 8H1.4M12.7 3.3l-1 1M4.3 11.7l-1 1M12.7 12.7l-1-1M4.3 4.3l-1-1" />
    </Svg>
  ),
  Moon: (p: Props) => (
    <Svg {...p}>
      <path d="M13.2 9.6A5.6 5.6 0 0 1 6.4 2.8a5.6 5.6 0 1 0 6.8 6.8z" />
    </Svg>
  ),
  Key: (p: Props) => (
    <Svg {...p}>
      <circle cx="5.2" cy="10.8" r="2.8" />
      <path d="m7.4 8.8 5.4-5.4M11 5.2l1.4 1.4M9.6 6.6 11 8" />
    </Svg>
  ),
  Alert: (p: Props) => (
    <Svg {...p}>
      <path d="M8 2.6 14.2 13H1.8z" />
      <path d="M8 6.4v3M8 11.4v.2" />
    </Svg>
  ),
  Info: (p: Props) => (
    <Svg {...p}>
      <circle cx="8" cy="8" r="6" />
      <path d="M8 7.4v3.4M8 5.2v.2" />
    </Svg>
  ),
  External: (p: Props) => (
    <Svg {...p}>
      <path d="M12.8 9.4v2.8a1.4 1.4 0 0 1-1.4 1.4H3.8a1.4 1.4 0 0 1-1.4-1.4V4.6a1.4 1.4 0 0 1 1.4-1.4h2.8" />
      <path d="M9.6 2.4h4v4M13.6 2.4 7.4 8.6" />
    </Svg>
  ),
  Relay: (p: Props) => (
    <Svg {...p}>
      <circle cx="8" cy="8" r="1.6" />
      <path d="M4.9 4.9a4.4 4.4 0 0 0 0 6.2M11.1 11.1a4.4 4.4 0 0 0 0-6.2" />
      <path d="M2.7 2.7a7.5 7.5 0 0 0 0 10.6M13.3 13.3a7.5 7.5 0 0 0 0-10.6" />
    </Svg>
  ),
  Lock: (p: Props) => (
    <Svg {...p}>
      <rect x="3.2" y="7" width="9.6" height="7" rx="1.6" />
      <path d="M5.6 7V5.2a2.4 2.4 0 0 1 4.8 0V7" />
    </Svg>
  ),
  Users: (p: Props) => (
    <Svg {...p}>
      <path d="M8 1.8 13 3.6v4.2c0 3.1-2 5.3-5 6.4-3-1.1-5-3.3-5-6.4V3.6z" />
      <circle cx="8" cy="7" r="1.6" />
      <path d="M5.6 11.6c.5-1.1 1.4-1.7 2.4-1.7s1.9.6 2.4 1.7" />
    </Svg>
  ),
  Mailbox: (p: Props) => (
    <Svg {...p}>
      <path d="M2 5.6 8 2l6 3.6v6.2a1.2 1.2 0 0 1-1.2 1.2H3.2A1.2 1.2 0 0 1 2 11.8z" />
      <path d="m2 5.8 6 3.8 6-3.8" />
    </Svg>
  ),
  /** Attività: what is moving, both ways. Vertical rather than the rail's
   *  left-and-right pair — on a phone the two directions are one list sorted by
   *  time, and the glyph says "up and down" because that is what the list does. */
  Activity: (p: Props) => (
    <Svg {...p}>
      <path d="M5 13V3M2.5 5.5 5 3l2.5 2.5" />
      <path d="M11 3v10M8.5 10.5 11 13l2.5-2.5" />
    </Svg>
  ),
  /** Scambia: a viewfinder, not a QR. The disc in the middle of the bar opens a
   *  camera, and the thing a camera does is frame something. */
  Scan: (p: Props) => (
    <Svg {...p}>
      <path d="M2 5V3a1 1 0 0 1 1-1h2M11 2h2a1 1 0 0 1 1 1v2M14 11v2a1 1 0 0 1-1 1h-2M5 14H3a1 1 0 0 1-1-1v-2" />
      <path d="M4.5 8h7" />
    </Svg>
  ),
  /** Link e depositi: an in-tray. What is left on a relay for somebody to come
   *  and take, which is a tray rather than a chain. */
  Tray: (p: Props) => (
    <Svg {...p}>
      <path d="M2 9h3.5l1 2h3l1-2H14M2 9l2-6h8l2 6v4H2z" />
    </Svg>
  ),
  /** One of your machines, by what kind it is. `DeviceDto.platform` is the only
   *  thing that picks between these, and it is for an icon and never a decision
   *  — see the type. */
  Laptop: (p: Props) => (
    <Svg {...p}>
      <rect x="1.5" y="3" width="13" height="8" rx="1.2" />
      <path d="M1 13h14" />
    </Svg>
  ),
  Desktop: (p: Props) => (
    <Svg {...p}>
      <rect x="1.5" y="2.5" width="13" height="9" rx="1.2" />
      <path d="M5.5 14h5M8 11.5V14" />
    </Svg>
  ),
  Phone: (p: Props) => (
    <Svg {...p}>
      <rect x="2" y="1.5" width="12" height="13" rx="1.5" />
      <path d="M6 12.5h4" />
    </Svg>
  ),
  Command: (p: Props) => (
    <Svg {...p}>
      <path d="M5.6 2.4a1.8 1.8 0 1 0 1.8 1.8v7.6a1.8 1.8 0 1 0 1.8-1.8H4.2a1.8 1.8 0 1 0 1.8 1.8V4.2a1.8 1.8 0 1 0-1.8 1.8h7.6a1.8 1.8 0 1 0-1.8-1.8z" />
    </Svg>
  ),
  /* -- Sezioni 13–16 ------------------------------------------------------ */
  /** Le opzioni della casella (14b): due cursori. */
  Sliders: (p: Props) => (
    <Svg {...p}>
      <path d="M3 4.5h10M3 11.5h10" />
      <circle cx="6" cy="4.5" r="1.6" />
      <circle cx="10.5" cy="11.5" r="1.6" />
    </Svg>
  ),
  /** Quello che parte: su e a destra. */
  ArrowUpRight: (p: Props) => (
    <Svg {...p}>
      <path d="M4 12 12 4M6.5 4H12v5.5" />
    </Svg>
  ),
  /** Quello che arriva: giù e a sinistra. */
  ArrowDownLeft: (p: Props) => (
    <Svg {...p}>
      <path d="M12 4 4 12M4 6.5V12h5.5" />
    </Svg>
  ),
  /** Il link pubblico: un globo. */
  Globe: (p: Props) => (
    <Svg {...p}>
      <circle cx="8" cy="8" r="6" />
      <path d="M2 8h12M8 2c2 2 2 10 0 12M8 2c-2 2-2 10 0 12" />
    </Svg>
  ),
  /** Arriva adesso, diretto: un fulmine. */
  Bolt: (p: Props) => (
    <Svg {...p}>
      <path d="M9 1.5 4 9h4l-1 5.5L12 7H8z" />
    </Svg>
  ),
  ChevronUp: (p: Props) => (
    <Svg {...p}>
      <path d="M3.5 10 8 5.5l4.5 4.5" />
    </Svg>
  ),
  Tablet: (p: Props) => (
    <Svg {...p}>
      <rect x="2" y="1.5" width="12" height="13" rx="1.5" />
      <path d="M6 12.5h4" />
    </Svg>
  ),
  /** Diretto, da qui a lì: le onde. */
  Wifi: (p: Props) => (
    <Svg {...p}>
      <path d="M2 6.5a9 9 0 0 1 12 0M4.2 9a5.6 5.6 0 0 1 7.6 0M6.4 11.4a2.3 2.3 0 0 1 3.2 0" />
      <circle cx="8" cy="13.4" r="0.6" fill="currentColor" />
    </Svg>
  ),
  /** In casella: un vassoio. */
  Inbox: (p: Props) => (
    <Svg {...p}>
      <path d="M2 9h3.5l1 2h3l1-2H14M2 9l2-6h8l2 6v4H2z" />
    </Svg>
  ),
  Clock: (p: Props) => (
    <Svg {...p}>
      <circle cx="8" cy="8" r="6" />
      <path d="M8 4.5V8l2.5 1.5" />
    </Svg>
  ),
  Download: (p: Props) => (
    <Svg {...p}>
      <path d="M8 2.5v8M4.5 7 8 10.5 11.5 7M3 13.5h10" />
    </Svg>
  ),
};

export type IconName = keyof typeof Icon;
