import os,sys,tempfile,unittest,socket
from pathlib import Path
from unittest.mock import patch
base=Path(__file__).resolve().parents[2]
sys.path[:0]=[str(base/'app'),str(base/'support/tests')]
tmp=tempfile.TemporaryDirectory(prefix='tibrary-release-')
os.environ['HOME']=tmp.name
os.environ['XDG_CONFIG_HOME']=tmp.name
if not os.environ.get('TIBRARY_NATIVE_TEST'):os.environ['QT_QPA_PLATFORM']='offscreen'
for k in ('TIDAL_CLIENT_ID','TIDAL_CLIENT_SECRET'):os.environ.pop(k,None)
from PySide6.QtCore import QSettings
QSettings.setDefaultFormat(QSettings.Format.IniFormat)
QSettings.setPath(QSettings.Format.IniFormat,QSettings.Scope.UserScope,tmp.name)
import library_manager
library_manager.__path__.append(str(base/'archive/qt/library_manager'))
from library_manager.credentials import Credentials
class MemoryKeys:
 def __init__(self):self.data={}
 def get_password(self,s,a):return self.data.get((s,a))
 def set_password(self,s,a,v):self.data[s,a]=v
 def delete_password(self,s,a):self.data.pop((s,a),None)
Credentials.__init__.__defaults__=(MemoryKeys,)
real_connect=socket.socket.connect
def connect(s,address):
 if isinstance(address,tuple) and address[0] not in ('127.0.0.1','localhost','::1'):raise OSError('External networking disabled in release tests')
 return real_connect(s,address)
# Never allow any test write to the external music drive.
def audit(event,args):
 if event=='open':
  path,mode,flags=args
  if isinstance(path,(str,bytes)) and os.fsdecode(path).startswith('/Volumes/') and (flags & (os.O_WRONLY|os.O_RDWR|os.O_CREAT|os.O_TRUNC|os.O_APPEND)):
   raise PermissionError('Release tests cannot write to external volumes')
 if event in ('os.remove','os.rename','os.rmdir','os.mkdir','os.chmod','os.utime') and any(isinstance(p,(str,bytes)) and os.fsdecode(p).startswith('/Volumes/') for p in args):raise PermissionError('External volume mutation blocked')
sys.addaudithook(audit)
with patch.object(socket.socket,'connect',connect):
 if len(sys.argv)>1 and sys.argv[1].endswith('.py'):
  import runpy
  test_file=sys.argv[1]
  sys.argv=[test_file]
  runpy.run_path(str(base/'support/tests'/test_file),run_name='__main__')
 else:
  suite=unittest.defaultTestLoader.loadTestsFromNames(sys.argv[1:]) if len(sys.argv)>1 else unittest.defaultTestLoader.discover(str(base/'support/tests'))
  result=unittest.TextTestRunner(verbosity=2).run(suite)
  sys.stdout.flush();sys.stderr.flush()
  os._exit(0 if result.wasSuccessful() else 1)
