"""A package whose extension needs a system library that isn't there, as opencv-python's needs
libGL.so.1 (tests/engines/py/system_library.v2): the error Linux's loader gives."""
raise ImportError("libZzGL.so.1: cannot open shared object file: No such file or directory")
