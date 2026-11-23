param(
    [Parameter(Mandatory = $true)]
    [string]$Installer,

    [string]$KeyFile = "$PSScriptRoot\src-tauri\ciyuan-proxy.key",

    [string]$Password
)

if (-not (Test-Path $Installer)) {
    Write-Error "安装包不存在: $Installer"
    exit 1
}

if (-not (Test-Path $KeyFile)) {
    Write-Error "签名私钥不存在: $KeyFile"
    exit 1
}

Set-Location "$PSScriptRoot"

# 私钥密码来源优先级:
#   1. -Password 参数
#   2. 环境变量 TAURI_SIGNING_PRIVATE_KEY_PASSWORD
#   3. 与私钥同名的 .pass 文件(ciyuan-proxy.key → ciyuan-proxy.key.pass,已被 gitignore)
#   4. 都没有 → tauri CLI 交互式提示
if (-not $Password) {
    $Password = $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD
}

if (-not $Password) {
    $passFile = "$KeyFile.pass"
    if (Test-Path $passFile) {
        $Password = (Get-Content -LiteralPath $passFile -Raw).Trim()
        Write-Host "已从密码文件读取: $passFile"
    }
}

Write-Host "正在签名: $Installer"
Write-Host "使用私钥: $KeyFile"

# 注意:不要用 `tauri signer sign --private-key <path>`。该参数走的是
# "文件内容就是 base64 私钥"的解析路径,与 `tauri signer generate -w` 产出的
# 文件格式(整份 minisign 文本再 base64)不兼容,会报
# "failed to decode base64 secret key"。走环境变量才是官方支持的用法。
$env:TAURI_SIGNING_PRIVATE_KEY_PATH = (Resolve-Path $KeyFile).Path
if ($Password) {
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $Password
} else {
    Remove-Item Env:\TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
}

npx tauri signer sign $Installer

if ($LASTEXITCODE -ne 0) {
    Write-Error "签名失败"
    exit $LASTEXITCODE
}

$sigFile = "$Installer.sig"
if (Test-Path $sigFile) {
    Write-Host ""
    Write-Host "签名文件已生成: $sigFile"
    Write-Host "签名内容:"
    Get-Content $sigFile
    Write-Host ""
    Write-Host "请将该路径填入数据库 version 表的 signature_path 字段"
} else {
    Write-Error "未找到签名文件: $sigFile"
    exit 1
}
