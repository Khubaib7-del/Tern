# Included after every `project()` in whisper.cpp's CMake build
# (CMAKE_PROJECT_INCLUDE, set in .cargo/config.toml and forwarded by
# whisper-rs-sys, which passes every CMAKE_* variable to CMake).
#
# Why: with MSVC and the Visual Studio generator, the `cmake` crate sets
# CMAKE_<LANG>_FLAGS_RELEASE (and _RELWITHDEBINFO) to the cc crate's flags
# with every /O flag removed ("let cmake deal with optimization"), which
# replaces CMake's own "/O2 /Ob2 /DNDEBUG". whisper.cpp was then compiled
# without optimisation on Windows: about 115 s for a 12 s clip with Whisper
# small, against about 8 s on Linux. Put the optimisation back.
if(MSVC)
  foreach(lang C CXX)
    foreach(cfg RELEASE RELWITHDEBINFO)
      if(NOT "${CMAKE_${lang}_FLAGS_${cfg}}" MATCHES "[/-]O[12x]")
        set(CMAKE_${lang}_FLAGS_${cfg} "${CMAKE_${lang}_FLAGS_${cfg}} /O2 /Ob2 /DNDEBUG")
      endif()
    endforeach()
  endforeach()
  message(STATUS "Tern: MSVC C flags (Release) = ${CMAKE_C_FLAGS_RELEASE}")
endif()
