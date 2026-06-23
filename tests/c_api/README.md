# GraphDB C API integration testing

This directory contains integration tests for GraphDB C API.

##Directory structure

```
tests/c_api/
├── tests.c              # C Test Source Code
├── CMakeLists.txt       # CMake Build Configuration
├── build_msvc.ps1       # MSVC build script (recommended)
└── README.md            #This document
```

##Preconditions

1. **Rust Toolchain **: Used to compile GraphDB library
2. **MSVC Compiler **: Windows platform recommends using Visual Studio's MSVC toolchain
3. **GraphDB library compiled **: Run `cargo build --lib` to generate library file

##Construction method

###Method 1: Use PowerShell scripts (recommended, Windows)

```powershell
#Enter the project root directory
cd graphDB

#Build and Run Tests
.\tests\c_api\build_msvc.ps1 -Run

#Build only (debug mode)
.\tests\c_api\build_msvc.ps1

#Build release version
.\tests\c_api\build_msvc.ps1 -BuildMode release

Clean and rebuild
.\tests\c_api\build_msvc.ps1 -Clean -Run
```

###Method 2: Use CMake

```bash
#Enter the test directory
cd tests/c_api

#Create build directory
mkdir build
cd build

#Configure CMake
cmake ..

#Build
cmake --build .

#Run tests
ctest --verbose
```

###Method 3: Manual compilation (MSVC)

```cmd
cl.exe /W4 /I../../include /Febuild\bin\graphdb_c_api_tests.exe tests.c /link /LIBPATH:../../target/debug graphdb.dll.lib ws2_32.lib
```

##Test coverage

###Database Life Cycle Testing
- Database Open/Close
- Library version acquisition
- null parameter processing

###Session Management Test
- Session Creation/Destruction
- autocommit mode
- null parameter processing

###Query Execution Test
- Simple query execution
- null parameter processing

###Result Handling Test
- Result Set Metadata (Number of Columns, Number of Rows)
- null parameter processing

###Transaction Management Test
- Transaction Start/Submit
- Transaction Start/Rollback
- null parameter processing

###Batch Operation Test
- Batch Inserter Create/Release
- null parameter processing

###Error Handling Test
- error code conversion
- Error Description Get
- Error message acquisition

###Integration Scenario Test
- complete workflow

##Frequently Asked Questions

### 1. GraphDB library not found

** Solution **: Compile GraphDB project first
```bash
cargo build --lib
```

### 2. DLL not found at runtime

** Solution **: Add library directory to PATH
```powershell
$env:PATH = "target\debug;$env:PATH"
```

### 3. Compilation error C2016

** Cause **: Empty structure exists in header file

** Resolution **: Ensure that the struct definition in `include/graphdb.h` contains `_dummy` members

##Notes

- Tests compiled using MSVC toolchain ensure Visual Studio environment variables are configured
- The test program automatically cleans up the generated test database files
- All test cases are independent tests and can be run independently
