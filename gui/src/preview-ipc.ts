// Dev-only: the tests' fake bridge, handed to the browser preview.
import { makeIpcMock } from "./test/mocks";
import { asAcceptError } from "./ipc";

const mock = makeIpcMock();
export const api = mock.api;
export const {
  onEngineEvent,
  onConnected,
  onDaemonError,
  onPickedFiles,
  onSharedArrived,
  onArvoloTicket,
  onUpdateProgress,
} = mock as unknown as Record<string, never> & typeof mock;
export { asAcceptError };
export type { AcceptError } from "./ipc";
