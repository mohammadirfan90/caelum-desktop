"""
CAELUM Desktop: Cryptographic Signer for Model Packages (CR-012, CR-018).

Provides Ed25519 signing and envelope generation for manifest.sig.toml,
matching caelum-alarms/src/signing.rs and caelum-ml/src/package.rs.
"""

import hashlib
import os
import struct
import toml
from cryptography.hazmat.primitives.asymmetric import ed25519
from cryptography.hazmat.primitives import serialization


class CaelumSigner:
    """Ed25519 signer implementing the CAELUM-CONFIG-1 domain separation."""
    CONFIG_TAG = b"CAELUM-CONFIG-1"

    def __init__(self, private_key: ed25519.Ed25519PrivateKey = None):
        if private_key is None:
            self.private_key = ed25519.Ed25519PrivateKey.generate()
        else:
            self.private_key = private_key
        self.public_key = self.private_key.public_key()

    @classmethod
    def from_seed(cls, seed_bytes: bytes):
        """Create signer from 32-byte raw private key seed."""
        private_key = ed25519.Ed25519PrivateKey.from_private_bytes(seed_bytes)
        return cls(private_key)

    @classmethod
    def from_hex(cls, hex_str: str):
        """Create signer from 64-character hex seed."""
        return cls.from_seed(bytes.fromhex(hex_str.strip()))

    def get_private_key_hex(self) -> str:
        raw_bytes = self.private_key.private_bytes(
            encoding=serialization.Encoding.Raw,
            format=serialization.PrivateFormat.Raw,
            encryption_algorithm=serialization.NoEncryption()
        )
        return raw_bytes.hex()

    def get_public_key_bytes(self) -> bytes:
        return self.public_key.public_bytes(
            encoding=serialization.Encoding.Raw,
            format=serialization.PublicFormat.Raw
        )

    def get_public_key_hex(self) -> str:
        return self.get_public_key_bytes().hex()

    def get_key_id(self) -> str:
        """First 8 bytes of SHA256 of the public key, formatted as 16 hex chars."""
        pub_bytes = self.get_public_key_bytes()
        digest = hashlib.sha256(pub_bytes).digest()
        return digest[:8].hex()

    def sign_config(self, kind: str, manifest_bytes: bytes, version: str) -> dict:
        """
        Signs manifest_bytes under kind and version, returning the dictionary
        representation of ConfigEnvelope (manifest.sig.toml).
        """
        manifest_sha256 = hashlib.sha256(manifest_bytes).hexdigest()

        # Framing: TAG + len(part)[8B BE] + part for kind, sha256, version
        msg = bytearray(self.CONFIG_TAG)
        for part in [kind, manifest_sha256, version]:
            part_bytes = part.encode("utf-8")
            msg.extend(struct.pack(">Q", len(part_bytes)))
            msg.extend(part_bytes)

        signature_bytes = self.private_key.sign(bytes(msg))

        envelope = {
            "kind": kind,
            "version": version,
            "sha256": manifest_sha256,
            "signature": [
                {
                    "role": "author",
                    "key_id": self.get_key_id(),
                    "signature": signature_bytes.hex()
                }
            ]
        }
        return envelope


def package_model(
    output_dir: str,
    manifest_dict: dict,
    onnx_filename: str,
    model_bytes: bytes,
    signer: CaelumSigner
) -> str:
    """Packages model ONNX file, manifest.toml, and manifest.sig.toml."""
    os.makedirs(output_dir, exist_ok=True)

    # 1. Save ONNX model
    onnx_path = os.path.join(output_dir, onnx_filename)
    with open(onnx_path, "wb") as f:
        f.write(model_bytes)

    model_sha256 = hashlib.sha256(model_bytes).hexdigest()
    manifest_dict["model"]["file"] = onnx_filename
    manifest_dict["model"]["sha256"] = model_sha256

    # 2. Save manifest.toml
    manifest_str = toml.dumps(manifest_dict)
    manifest_bytes = manifest_str.encode("utf-8")
    manifest_path = os.path.join(output_dir, "manifest.toml")
    with open(manifest_path, "wb") as f:
        f.write(manifest_bytes)

    # 3. Sign and save manifest.sig.toml
    envelope = signer.sign_config(
        kind="model",
        manifest_bytes=manifest_bytes,
        version=manifest_dict["model"]["version"]
    )
    sig_path = os.path.join(output_dir, "manifest.sig.toml")
    with open(sig_path, "w", encoding="utf-8") as f:
        toml.dump(envelope, f)

    return model_sha256
