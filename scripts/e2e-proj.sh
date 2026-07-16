#!/usr/bin/env bash
# End-to-end checks against a real second peer and the real relay.
#
# The unit suites prove the pieces; this proves the product. It drives the actual
# daemon on this machine against the actual daemon on `proj`, over the public
# relay — the path a user's file really takes. Everything it asserts is something
# a user would notice: the file arrives, its bytes are intact, the sender learns
# it landed, a cancelled send really is withdrawn.
#
# Usage:  scripts/e2e-proj.sh            (needs: local daemon, ssh to $PEER_HOST)
# Each case is independent and cleans up after itself.

set -uo pipefail

PEER_HOST="${PEER_HOST:-root@46.225.74.132}"
PEER_NAME="${PEER_NAME:-proj}"       # our contact name for them
SELF_NAME="${SELF_NAME:-mac}"        # their contact name for us
ARVOLO="${ARVOLO:-arvolo}"
TMP="$(mktemp -d)"
PASS=0; FAIL=0

cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

say()  { printf '\n\033[1m▸ %s\033[0m\n' "$*"; }
ok()   { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$*"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[31m✗ %s\033[0m\n' "$*"; }

peer() { ssh -o ConnectTimeout=8 -o BatchMode=yes "$PEER_HOST" "$@" 2>/dev/null; }

# Wait until `$1` (a shell test) holds, up to $2 seconds.
wait_for() {
  local cond="$1" secs="${2:-60}" i=0
  while [ "$i" -lt "$((secs*2))" ]; do
    if eval "$cond" >/dev/null 2>&1; then return 0; fi
    sleep 0.5; i=$((i+1))
  done
  return 1
}

# The offer id `proj` is holding for a file named $1 (empty if none).
peer_offer_for() {
  peer "$ARVOLO transfers 2>&1 | grep -F '$1' | grep -o 'arvolo accept [a-z0-9]*' | awk '{print \$3}' | head -1"
}
# The offer id *we* are holding for a file named $1.
self_offer_for() {
  $ARVOLO transfers 2>&1 | grep -F "$1" | grep -o 'arvolo accept [a-z0-9]*' | awk '{print $3}' | head -1
}
self_status_of() { $ARVOLO transfers 2>&1 | grep -F "$1" | head -1; }

require() {
  command -v "$ARVOLO" >/dev/null || { echo "no $ARVOLO on PATH"; exit 1; }
  $ARVOLO transfers >/dev/null 2>&1 || { echo "local daemon not answering"; exit 1; }
  peer "$ARVOLO transfers" >/dev/null || { echo "peer daemon not answering"; exit 1; }
}

# ---------------------------------------------------------------- cases

# The everyday path: they are online, so it goes straight over P2P.
case_live_send() {
  say "invio live P2P (destinatario online)"
  local f="live-$$.txt" body="live $(date +%s)"
  echo "$body" > "$TMP/$f"
  $ARVOLO send --to "$PEER_NAME" "$TMP/$f" >/dev/null 2>&1

  local id; id=$(wait_for "[ -n \"\$(peer_offer_for $f)\" ]" 60 && peer_offer_for "$f")
  [ -n "$id" ] && ok "l'offerta raggiunge il peer" || { bad "nessuna offerta sul peer"; return; }

  peer "$ARVOLO accept $id" >/dev/null
  if wait_for "peer \"cat ~/Arvolo/$f\" | grep -qF '$body'" 90; then
    ok "il file arriva con i byte giusti"
  else
    bad "il file non è arrivato integro"
  fi
  # The sender must conclude on its own — this is the bug that left sends at 0%.
  if wait_for "$ARVOLO transfers 2>&1 | grep -F '$f' | grep -qE 'completed|deposited'"; then
    ok "il mittente conclude da solo: $(self_status_of "$f" | sed 's/.*(\(.*\))/\1/')"
  else
    bad "il mittente non conclude (resta $(self_status_of "$f"))"
  fi
  peer "rm -f ~/Arvolo/$f" >/dev/null
}

# They are offline: the file must wait on the relay and land when they return.
case_mailbox_send() {
  say "invio a destinatario offline (mailbox) e ripresa al ritorno"
  local f="mbox-$$.txt" body="mailbox $(date +%s)"
  echo "$body" > "$TMP/$f"

  peer "systemctl stop arvolo" >/dev/null; sleep 3
  $ARVOLO send --to "$PEER_NAME" "$TMP/$f" >/dev/null 2>&1

  if wait_for "$ARVOLO transfers 2>&1 | grep -F '$f' | grep -q deposited" 120; then
    ok "depositato sul relay mentre il destinatario è offline"
  else
    bad "non depositato (stato: $(self_status_of "$f"))"
  fi

  peer "systemctl start arvolo" >/dev/null
  local id; id=$(wait_for "[ -n \"\$(peer_offer_for $f)\" ]" 90 && peer_offer_for "$f")
  [ -n "$id" ] && ok "al ritorno il peer trova l'offerta in attesa" || { bad "offerta persa al ritorno"; return; }

  peer "$ARVOLO accept $id" >/dev/null
  if wait_for "peer \"cat ~/Arvolo/$f\" | grep -qF '$body'" 90; then
    ok "il file depositato viene ritirato integro"
  else
    bad "il ritiro dalla mailbox è fallito"
  fi
  peer "rm -f ~/Arvolo/$f" >/dev/null
}

# Cancelling a deposit must actually withdraw it — not just hide the row.
case_cancel_withdraws() {
  say "annullare un deposito lo ritira davvero dal relay"
  local f="cancel-$$.txt"
  echo "da annullare" > "$TMP/$f"

  peer "systemctl stop arvolo" >/dev/null; sleep 3
  $ARVOLO send --to "$PEER_NAME" "$TMP/$f" >/dev/null 2>&1
  wait_for "$ARVOLO transfers 2>&1 | grep -F '$f' | grep -q deposited" 120 \
    || { bad "non è arrivato a deposited, caso saltato"; peer "systemctl start arvolo" >/dev/null; return; }

  local n; n=$($ARVOLO transfers 2>&1 | grep -F "$f" | grep -oE '^\s*\[[0-9]+\]' | tr -dc '0-9')
  $ARVOLO cancel "$n" >/dev/null 2>&1
  sleep 4
  peer "systemctl start arvolo" >/dev/null; sleep 8

  if [ -z "$(peer_offer_for "$f")" ]; then
    ok "l'offerta è sparita dalla inbox del destinatario"
  else
    bad "l'offerta è ancora ritirabile dal destinatario"
  fi
}

# The direction the GUI's "Ricevuti" column exists for — never exercised before.
case_incoming() {
  say "ricezione: proj → questa macchina"
  local f="incoming-$$.txt" body="in arrivo $(date +%s)"
  peer "echo '$body' > /tmp/$f && $ARVOLO send --to $SELF_NAME /tmp/$f" >/dev/null 2>&1

  local id; id=$(wait_for "[ -n \"\$(self_offer_for $f)\" ]" 90 && self_offer_for "$f")
  [ -n "$id" ] && ok "l'offerta in arrivo appare qui" || { bad "nessuna offerta in arrivo"; return; }

  $ARVOLO accept "$id" >/dev/null 2>&1
  if wait_for "grep -qF '$body' ~/Arvolo/$f" 90; then
    ok "il file ricevuto è integro"
  else
    bad "il file ricevuto non è integro"
  fi
  rm -f ~/Arvolo/"$f"; peer "rm -f /tmp/$f" >/dev/null
}

# Refusing an arrival must not save anything.
case_reject() {
  say "rifiutare un arrivo non salva nulla"
  local f="reject-$$.txt"
  peer "echo rifiutami > /tmp/$f && $ARVOLO send --to $SELF_NAME /tmp/$f" >/dev/null 2>&1

  local id; id=$(wait_for "[ -n \"\$(self_offer_for $f)\" ]" 90 && self_offer_for "$f")
  [ -n "$id" ] || { bad "nessuna offerta da rifiutare"; return; }

  $ARVOLO reject "$id" >/dev/null 2>&1
  sleep 3
  [ -z "$(self_offer_for "$f")" ] && ok "l'offerta è stata rimossa" || bad "l'offerta è ancora lì"
  [ ! -f ~/Arvolo/"$f" ] && ok "nessun file scritto su disco" || { bad "un file rifiutato è stato salvato"; rm -f ~/Arvolo/"$f"; }
  peer "rm -f /tmp/$f" >/dev/null
}

# A ticket is the no-recipient path: anyone holding it can fetch.
case_ticket() {
  say "ticket P2P: il peer scarica senza essere il destinatario"
  local f="ticket-$$.bin"
  head -c 200000 /dev/urandom > "$TMP/$f"
  local sum; sum=$(shasum -a 256 "$TMP/$f" | awk '{print $1}')

  local out; out=$($ARVOLO send "$TMP/$f" 2>&1)
  local tk; tk=$(echo "$out" | grep -oiE 'arvc[a-z0-9]+' | head -1)
  [ -n "$tk" ] && ok "il ticket è stato generato" || { bad "nessun ticket: $out"; return; }

  if peer "cd /tmp && $ARVOLO recv '$tk' >/dev/null 2>&1 && sha256sum /tmp/$f | cut -d' ' -f1" | grep -qi "$sum"; then
    ok "il peer scarica dal ticket e i byte coincidono"
  else
    bad "download da ticket fallito o corrotto"
  fi
  peer "rm -f /tmp/$f" >/dev/null
}

# A link is the no-Arvolo path: a browser (curl) must be able to take it.
case_link() {
  say "link pubblico: scaricabile da un browser, senza Arvolo"
  local f="link-$$.txt" body="scaricami $(date +%s)"
  echo "$body" > "$TMP/$f"
  local out; out=$($ARVOLO send --link "$TMP/$f" 2>&1)
  local url; url=$(echo "$out" | grep -oE 'https?://[^ ]+/dl/[^ ]+' | head -1)
  if [ -n "$url" ]; then
    # The key lives in the #fragment and never reaches the relay; assert the relay
    # serves the page a browser would open.
    local code; code=$(curl -sS -m 20 -o /dev/null -w '%{http_code}' "${url%%#*}")
    [ "$code" = "200" ] && ok "il relay serve il link (HTTP $code)" || bad "il link risponde $code"
  elif echo "$out" | grep -q "disabled by its administrator"; then
    # This relay turns links off. That is a choice, not a fault — what matters is
    # that we say so plainly instead of producing a link that cannot work.
    ok "link disabilitati dal relay: il rifiuto è chiaro e spiega l'alternativa"
  else
    bad "nessun link e nessuna spiegazione: $out"
  fi
}

# A folder must arrive as one archive, with its contents intact.
case_folder() {
  say "invio di una cartella (archiviata) con più file"
  local d="dir-$$"
  mkdir -p "$TMP/$d/sub"
  echo "uno" > "$TMP/$d/a.txt"; echo "due" > "$TMP/$d/sub/b.txt"
  $ARVOLO send --to "$PEER_NAME" "$TMP/$d" >/dev/null 2>&1

  local id; id=$(wait_for "[ -n \"\$(peer_offer_for $d)\" ]" 90 && peer_offer_for "$d")
  [ -n "$id" ] && ok "l'offerta della cartella arriva" || { bad "nessuna offerta per la cartella"; return; }

  peer "$ARVOLO accept $id" >/dev/null
  if wait_for "peer \"cat ~/Arvolo/$d/$d/sub/b.txt\" | grep -q due" 90; then
    ok "la cartella è ricostruita con la sua struttura"
  else
    bad "la cartella non è arrivata integra"
  fi
  peer "rm -rf ~/Arvolo/$d" >/dev/null
}

# A held send must stop trying when paused and pick up when resumed.
case_pause_resume() {
  say "pausa e ripresa di un invio in attesa"
  local f="pause-$$.txt"
  echo "in pausa" > "$TMP/$f"
  # Pause acts on a send that is still *trying*. A recipient who is online but has
  # not accepted yet leaves the send Active — the real moment a user reaches for
  # pause. (An offline recipient is no good here: with a working relay the send
  # deposits within seconds and a deposit is not pausable.)
  $ARVOLO send --to "$PEER_NAME" "$TMP/$f" >/dev/null 2>&1
  local n
  wait_for "$ARVOLO transfers 2>&1 | grep -F '$f' | grep -qE 'active|waiting'" 30
  n=$($ARVOLO transfers 2>&1 | grep -F "$f" | grep -oE '^\s*\[[0-9]+\]' | tr -dc '0-9' | head -1)
  [ -n "$n" ] || { bad "invio non trovato (stato: $(self_status_of "$f"))"; return; }

  $ARVOLO pause "$n" >/dev/null 2>&1
  if wait_for "$ARVOLO transfers 2>&1 | grep -F '$f' | grep -q paused" 40; then
    ok "l'invio si mette in pausa"
  else
    bad "non è andato in pausa (stato: $(self_status_of "$f"))"
  fi
  $ARVOLO resume "$n" >/dev/null 2>&1
  if wait_for "! $ARVOLO transfers 2>&1 | grep -F '$f' | grep -q paused" 40; then
    ok "l'invio riprende"
  else
    bad "non è ripreso"
  fi
  $ARVOLO cancel "$n" >/dev/null 2>&1
  # Clear the offer we left parked on the peer.
  local oid; oid=$(peer_offer_for "$f"); [ -n "$oid" ] && peer "$ARVOLO reject $oid" >/dev/null
}

# ---------------------------------------------------------------- run

require
say "peer: $PEER_HOST · relay: $($ARVOLO transfers 2>&1 | head -1 | grep -oE 'https?://[^ ]+')"

for c in "${@:-all}"; do :; done
if [ "${1:-all}" = "all" ]; then
  case_live_send
  case_incoming
  case_reject
  case_mailbox_send
  case_cancel_withdraws
  case_ticket
  case_link
  case_folder
  case_pause_resume
else
  "case_$1"
fi

printf '\n\033[1m%s passati, %s falliti\033[0m\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
