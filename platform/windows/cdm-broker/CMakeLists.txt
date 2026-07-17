cmake_minimum_required(VERSION 3.24)
project(ubar_cdm_appcontainer LANGUAGES CXX)

add_executable(ubar-cdm-appcontainer main.cpp)
target_compile_features(ubar-cdm-appcontainer PRIVATE cxx_std_20)
target_compile_definitions(ubar-cdm-appcontainer PRIVATE UNICODE _UNICODE WIN32_LEAN_AND_MEAN NOMINMAX _WIN32_WINNT=0x0A00)
target_link_libraries(ubar-cdm-appcontainer PRIVATE userenv advapi32)
