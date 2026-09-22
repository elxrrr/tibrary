"""Local failure traces without credentials, request bodies, or tag contents."""
import faulthandler
import logging
from logging.handlers import RotatingFileHandler
import sys
import traceback

logger=logging.getLogger('library_manager')
_fault_file=None


def record_failure(context,exc):
    # Deliberately omit exception messages/locals, which may contain provider secrets.
    frames=''.join(traceback.format_tb(exc.__traceback__))
    logger.error('%s · %s\n%s',context,type(exc).__name__,frames)


def install(directory):
    global _fault_file
    directory.mkdir(parents=True,exist_ok=True)
    handler=RotatingFileHandler(directory/'errors.log',maxBytes=1_000_000,backupCount=2)
    handler.setFormatter(logging.Formatter('%(asctime)s %(message)s'));logger.addHandler(handler);logger.setLevel(logging.INFO)
    _fault_file=open(directory/'faults.log','a')
    faulthandler.enable(file=_fault_file)
    def unhandled(kind,exc,tb):
        record_failure('Unhandled application callback',exc)
        sys.__excepthook__(kind,exc,tb)
    sys.excepthook=unhandled
