"""Keep UI tests away from the user's saved desktop preferences."""
import atexit
import tempfile
from PySide6.QtCore import QSettings

def isolate_settings():
    directory=tempfile.TemporaryDirectory();atexit.register(directory.cleanup)
    QSettings.setDefaultFormat(QSettings.Format.IniFormat)
    QSettings.setPath(QSettings.Format.IniFormat,QSettings.Scope.UserScope,directory.name)
