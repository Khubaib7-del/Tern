"""Checks that each Windows executable delay-loads vulkan-1.dll (#67).

A normal import of vulkan-1.dll stops the program from starting on a
machine without a Vulkan driver; a delayed one is loaded only when
whisper.cpp first asks for Vulkan. Prints every executable's imports and
fails when vulkan-1.dll is a normal import or missing from both lists.
"""
import sys

import pefile

ok = True
for path in sys.argv[1:]:
    pe = pefile.PE(path, fast_load=True)
    pe.parse_data_directories(
        directories=[
            pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"],
            pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_DELAY_IMPORT"],
        ]
    )
    normal = [e.dll.decode().lower() for e in getattr(pe, "DIRECTORY_ENTRY_IMPORT", [])]
    delayed = [e.dll.decode().lower() for e in getattr(pe, "DIRECTORY_ENTRY_DELAY_IMPORT", [])]
    print(f"{path}\n  imports: {', '.join(normal)}\n  delayed: {', '.join(delayed) or '-'}")
    for e in getattr(pe, "DIRECTORY_ENTRY_DELAY_IMPORT", []):
        if e.dll.decode().lower() == "vulkan-1.dll":
            names = [i.name.decode() for i in e.imports if i.name]
            print(f"  vulkan-1.dll functions: {', '.join(names)}")
    if "vulkan-1.dll" in normal:
        print("  ERROR: vulkan-1.dll is a normal import; Tern would not start without a Vulkan driver")
        ok = False
    elif "vulkan-1.dll" not in delayed:
        print("  ERROR: vulkan-1.dll is not imported at all (is the Vulkan speech build on?)")
        ok = False
sys.exit(0 if ok else 1)
