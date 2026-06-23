# GraphDB C API 测试构建脚本 (MSVC)
# 用于 Windows PowerShell 环境

param(
    [string]$BuildMode = "debug",
    [switch]$Clean = $false,
    [switch]$Run = $false
)

$ErrorActionPreference = "Stop"

# 设置 MSVC 环境变量
$vsPath = "D:\softwares\Visual Studio"
$vcVersion = "14.43.34808"
$windowsSdkPath = "C:\Program Files (x86)\Windows Kits\10"
$windowsSdkVersion = "10.0.22621.0"

# 基本路径
$env:Path = "$vsPath\VC\Tools\MSVC\$vcVersion\bin\Hostx64\x64;$env:Path"

# 包含路径 - MSVC + Windows SDK (UCRT + Shared + UM)
$env:INCLUDE = "$vsPath\VC\Tools\MSVC\$vcVersion\include;" +
               "$windowsSdkPath\Include\$windowsSdkVersion\ucrt;" +
               "$windowsSdkPath\Include\$windowsSdkVersion\shared;" +
               "$windowsSdkPath\Include\$windowsSdkVersion\um"

# 库路径 - MSVC + Windows SDK
$env:LIB = "$vsPath\VC\Tools\MSVC\$vcVersion\lib\x64;" +
           "$windowsSdkPath\Lib\$windowsSdkVersion\ucrt\x64;" +
           "$windowsSdkPath\Lib\$windowsSdkVersion\um\x64"

# 获取脚本所在目录
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent (Split-Path -Parent $ScriptDir)

# 设置路径
$IncludeDir = Join-Path $ProjectRoot "include"
$SourceDir = $ScriptDir
$BuildDir = Join-Path $ScriptDir "build"
$OutputDir = Join-Path $BuildDir "bin"
$LibDir = Join-Path $ProjectRoot "target\$BuildMode"

# 创建构建目录
if ($Clean -and (Test-Path $BuildDir)) {
    Write-Host "清理构建目录..." -ForegroundColor Yellow
    Remove-Item -Path $BuildDir -Recurse -Force
}

New-Item -ItemType Directory -Path $BuildDir -Force | Out-Null
New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  GraphDB C API 测试构建脚本 (MSVC)" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

Write-Host "配置信息:" -ForegroundColor Green
Write-Host "  项目根目录: $ProjectRoot"
Write-Host "  包含目录: $IncludeDir"
Write-Host "  源文件目录: $SourceDir"
Write-Host "  构建目录: $BuildDir"
Write-Host "  输出目录: $OutputDir"
Write-Host "  库目录: $LibDir"
Write-Host "  构建模式: $BuildMode"
Write-Host ""

# 检查库文件
$LibName = "graphdb.dll.lib"
$LibPath = Join-Path $LibDir $LibName

if (-not (Test-Path $LibPath)) {
    Write-Host "错误: 未找到 GraphDB 库文件: $LibPath" -ForegroundColor Red
    Write-Host "请先构建 GraphDB 项目: cargo build --$BuildMode" -ForegroundColor Red
    exit 1
}

Write-Host "找到库文件: $LibPath" -ForegroundColor Green
Write-Host ""

# 编译选项
$SourceFile = Join-Path $SourceDir "tests.c"
$OutputFile = Join-Path $OutputDir "graphdb_c_api_tests.exe"
$ObjectFile = Join-Path $BuildDir "tests.obj"

Write-Host "编译测试程序..." -ForegroundColor Cyan

# MSVC 编译命令
$CompileArgs = @(
    "/W4",
    "/I", $IncludeDir,
    "/Fo:$ObjectFile",
    "/Fe:$OutputFile",
    $SourceFile,
    "/link",
    "/LIBPATH:$LibDir",
    "graphdb.dll.lib",
    "ws2_32.lib"
)

Write-Host "执行命令: cl.exe $($CompileArgs -join ' ')" -ForegroundColor DarkGray
Write-Host "INCLUDE=$env:INCLUDE" -ForegroundColor DarkGray
Write-Host "LIB=$env:LIB" -ForegroundColor DarkGray
Write-Host ""

& cl.exe @CompileArgs

if ($LASTEXITCODE -ne 0) {
    Write-Host "编译失败!" -ForegroundColor Red
    exit $LASTEXITCODE
}

Write-Host "编译成功!" -ForegroundColor Green
Write-Host "输出文件: $OutputFile" -ForegroundColor Green
Write-Host ""

# 运行测试
if ($Run) {
    Write-Host "运行测试..." -ForegroundColor Cyan
    Write-Host ""
    
    # 设置库路径
    $env:PATH = "$LibDir;$env:PATH"
    
    & $OutputFile
    
    if ($LASTEXITCODE -ne 0) {
        Write-Host ""
        Write-Host "测试失败!" -ForegroundColor Red
        exit $LASTEXITCODE
    }
    
    Write-Host ""
    Write-Host "========================================" -ForegroundColor Green
    Write-Host "  所有测试通过!" -ForegroundColor Green
    Write-Host "========================================" -ForegroundColor Green
} else {
    Write-Host "提示: 使用 -Run 参数运行测试" -ForegroundColor Yellow
    Write-Host "示例: .\build_msvc.ps1 -Run" -ForegroundColor Yellow
}

Write-Host ""
