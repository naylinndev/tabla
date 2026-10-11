import { useState } from "react";
import { Check, Copy, Heart } from "lucide-react";
import qr from "../assets/donate-btc.svg";
import { Modal } from "./ui";

export const BTC_ADDRESS = "bc1qjssygyqayth4sm8zcw7kq56ml3rvl3vsm24qq9";

export function SupportDialog({ onClose }: { onClose(): void }) {
  const [copied, setCopied] = useState(false);
  return (
    <Modal onClose={onClose}>
      <div className="modal-head">
        <div className="ai-badge" style={{ background: "linear-gradient(180deg, #ff7a8a, #e0294a)" }}>
          <Heart />
        </div>
        <div>
          <h2>Support Tabla</h2>
          <p>Tabla is free and always will be. Donations help keep it that way.</p>
        </div>
      </div>
      <div className="modal-body support">
        <img src={qr} alt="Bitcoin donation QR code" className="support-qr" />
        <div className="support-addr mono">{BTC_ADDRESS}</div>
        <button
          className="btn primary"
          onClick={async () => {
            await navigator.clipboard.writeText(BTC_ADDRESS);
            setCopied(true);
            setTimeout(() => setCopied(false), 1800);
          }}
        >
          {copied ? <Check /> : <Copy />} {copied ? "Address copied" : "Copy Bitcoin address"}
        </button>
        <p className="hint" style={{ textAlign: "center", justifyContent: "center" }}>
          On-chain Bitcoin only (native SegWit). Always check the address in your wallet before sending.
        </p>
      </div>
    </Modal>
  );
}
