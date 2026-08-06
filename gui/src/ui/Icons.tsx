// One icon set, drawn rather than imported.
//
// Every glyph is a 16×16 stroked path on `currentColor`, so an icon inherits the
// colour of whatever it sits in and needs no dark-mode variant. They are inline
// SVG for the same reason the app has no CDN: a Tauri window with a strict CSP
// cannot fetch an icon font, and shipping one as a binary would put 200 glyphs in
// the bundle to use thirty.
//
// The geometry is deliberately plain — 1.6px strokes, round joins, 2px inset —
// because these sit next to 12px labels and any more detail turns to mud.

interface Props {
  size?: number;
  className?: string;
  /** Decorative by default; pass a label when the icon *is* the button. */
  label?: string;
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
      strokeWidth={1.6}
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
  Settings: (p: Props) => (
    <Svg {...p}>
      <circle cx="8" cy="8" r="2.1" />
      <path d="M8 1.6v1.6M8 12.8v1.6M14.4 8h-1.6M3.2 8H1.6M12.5 3.5l-1.1 1.1M4.6 11.4l-1.1 1.1M12.5 12.5l-1.1-1.1M4.6 4.6 3.5 3.5" />
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
  Command: (p: Props) => (
    <Svg {...p}>
      <path d="M5.6 2.4a1.8 1.8 0 1 0 1.8 1.8v7.6a1.8 1.8 0 1 0 1.8-1.8H4.2a1.8 1.8 0 1 0 1.8 1.8V4.2a1.8 1.8 0 1 0-1.8 1.8h7.6a1.8 1.8 0 1 0-1.8-1.8z" />
    </Svg>
  ),
};

export type IconName = keyof typeof Icon;
