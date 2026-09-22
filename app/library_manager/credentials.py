"""Credential pairs live in memory or the OS credential store, never SQLite."""
import json
import os
import sys


class CredentialError(Exception):
    pass


def system_backend():
    # Select an OS backend explicitly; never fall back to a plaintext plugin.
    try:
        if sys.platform == 'darwin':
            from keyring.backends.macOS import Keyring
        elif sys.platform == 'win32':
            from keyring.backends.Windows import WinVaultKeyring as Keyring
        else:
            from keyring.backends.SecretService import Keyring
        return Keyring()
    except Exception:
        raise CredentialError('Secure credential storage is unavailable. Use credentials for this session only.') from None


class Credentials:
    SERVICE = 'Tibrary'
    LEGACY_SERVICE = 'TidalLibraryManager'
    ACCOUNT = 'catalogue-client'

    def __init__(self, backend_factory=system_backend):
        self.backend_factory = backend_factory
        self.session = None
        self.source = 'Not loaded'
        # Keep every credential seen during this process redacted, including a
        # pair that has just been replaced or failed to save.
        self._redacted_values = set()

    def _remember_for_redaction(self, *values):
        self._redacted_values.update(str(value) for value in values if value)

    def get(self):
        if self.session:
            return self.session
        try:
            backend = self.backend_factory()
            stored = backend.get_password(self.SERVICE, self.ACCOUNT)
            if not stored:
                stored = backend.get_password(self.LEGACY_SERVICE, self.ACCOUNT)
                if stored:
                    try:
                        backend.set_password(self.SERVICE, self.ACCOUNT, stored)
                    except Exception:
                        pass
            if stored:
                data = json.loads(stored)
                if not all(isinstance(data.get(k), str) and data[k].strip() for k in ('id', 'secret')):
                    raise ValueError('Invalid saved credentials')
                self.session = (data['id'], data['secret'])
                self._remember_for_redaction(*self.session)
                self.source = 'OS credential store'
                return self.session
        except Exception:
            # Environment credentials remain useful on machines without a keychain.
            if not (os.getenv('TIDAL_CLIENT_ID') and os.getenv('TIDAL_CLIENT_SECRET')):
                raise CredentialError('Could not read the OS credential store. Open Settings → Connections to enter credentials or unlock your keychain.') from None
        pair = (os.getenv('TIDAL_CLIENT_ID'), os.getenv('TIDAL_CLIENT_SECRET'))
        if all(pair):
            self.session = pair
            self._remember_for_redaction(*pair)
            self.source = 'Environment'
            return pair
        raise CredentialError('No TIDAL credentials configured. Open Settings → Connections and enter your client ID and secret.')

    def has_keys(self):
        try:
            pair = self.get()
            return bool(pair and pair[0] and pair[1])
        except Exception:
            return False

    def save(self, client, secret, remember=True):
        client, secret = client.strip(), secret.strip()
        self._remember_for_redaction(client, secret)
        if not client or not secret:
            raise CredentialError('Enter both a client ID and a client secret.')
        if remember:
            try:
                self.backend_factory().set_password(self.SERVICE, self.ACCOUNT, json.dumps({'id': client, 'secret': secret}))
            except Exception:
                raise CredentialError('Credentials were not saved. Unlock your keychain or choose session-only storage.') from None
        self.session = (client, secret)
        self.source = 'OS credential store' if remember else 'This session only'
        return 'Credentials saved securely.' if remember else 'Credentials set for this session. Any previously saved credentials remain unchanged.'

    def forget(self):
        try:
            backend = self.backend_factory()
            if backend.get_password(self.SERVICE, self.ACCOUNT) is not None:
                backend.delete_password(self.SERVICE, self.ACCOUNT)
            if hasattr(self, 'LEGACY_SERVICE') and backend.get_password(self.LEGACY_SERVICE, self.ACCOUNT) is not None:
                try: backend.delete_password(self.LEGACY_SERVICE, self.ACCOUNT)
                except Exception: pass
        except Exception:
            raise CredentialError('Saved credentials could not be removed. Unlock your keychain and try again.') from None
        self.session = None
        self.source = 'Removed (environment fallback may still apply)'
        return 'Saved and session credentials removed. Environment variables, if configured, still apply.'

    def redact(self, message):
        message = str(message)
        values = list(self._redacted_values) + list(self.session or ()) + [os.getenv('TIDAL_CLIENT_ID'), os.getenv('TIDAL_CLIENT_SECRET')]
        for value in sorted((v for v in values if v), key=len, reverse=True):
            message = message.replace(value, '[redacted]')
        return message
