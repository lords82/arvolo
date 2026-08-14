import { Empty, Button, Icon } from "arvolo-gui";

export const NoTransfers = () => (
  <div style={{ width: 380 }}>
    <Empty icon={<Icon.Send />} title="Nothing in flight">
      Transfers you send or receive show up here while they run.
    </Empty>
  </div>
);

export const WithAction = () => (
  <div style={{ width: 380 }}>
    <Empty
      icon={<Icon.People />}
      title="No one paired yet"
      action={<Button variant="primary">Pair a device</Button>}
    >
      Pair a device to send files without typing a code every time.
    </Empty>
  </div>
);

export const TitleOnly = () => (
  <div style={{ width: 380 }}>
    <Empty icon={<Icon.History />} title="No history yet" />
  </div>
);
