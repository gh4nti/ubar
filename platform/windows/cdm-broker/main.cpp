#include <windows.h>
#include <userenv.h>
#include <aclapi.h>

#include <cstddef>
#include <cstdint>
#include <iostream>
#include <map>
#include <string>
#include <vector>

namespace {

std::wstring quote(const std::wstring& value)
{
    std::wstring output = L"\"";
    size_t slashes = 0;
    for (wchar_t character : value) {
        if (character == L'\\') {
            ++slashes;
        } else if (character == L'\"') {
            output.append(slashes * 2 + 1, L'\\');
            output.push_back(L'\"');
            slashes = 0;
        } else {
            output.append(slashes, L'\\');
            slashes = 0;
            output.push_back(character);
        }
    }
    output.append(slashes * 2, L'\\');
    output.push_back(L'\"');
    return output;
}

bool grant_read_execute(const std::wstring& path, PSID app_container)
{
    PACL existing = nullptr;
    PSECURITY_DESCRIPTOR descriptor = nullptr;
    DWORD result = GetNamedSecurityInfoW(
        const_cast<wchar_t*>(path.c_str()), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION,
        nullptr, nullptr, &existing, nullptr, &descriptor);
    if (result != ERROR_SUCCESS)
        return false;
    EXPLICIT_ACCESSW access {};
    access.grfAccessPermissions = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;
    access.grfAccessMode = GRANT_ACCESS;
    access.grfInheritance = NO_INHERITANCE;
    access.Trustee.TrusteeForm = TRUSTEE_IS_SID;
    access.Trustee.TrusteeType = TRUSTEE_IS_UNKNOWN;
    access.Trustee.ptstrName = static_cast<wchar_t*>(app_container);
    PACL updated = nullptr;
    result = SetEntriesInAclW(1, &access, existing, &updated);
    if (result == ERROR_SUCCESS) {
        result = SetNamedSecurityInfoW(
            const_cast<wchar_t*>(path.c_str()), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION,
            nullptr, nullptr, updated, nullptr);
    }
    if (updated) LocalFree(updated);
    if (descriptor) LocalFree(descriptor);
    return result == ERROR_SUCCESS;
}

PSID app_container_sid()
{
    constexpr wchar_t name[] = L"dev.ghanti.ubar.cdm";
    PSID sid = nullptr;
    HRESULT result = DeriveAppContainerSidFromAppContainerName(name, &sid);
    if (SUCCEEDED(result))
        return sid;
    result = CreateAppContainerProfile(
        name, L"uBar CDM", L"Isolated uBar content decryption worker", nullptr, 0, &sid);
    if (HRESULT_CODE(result) == ERROR_ALREADY_EXISTS)
        result = DeriveAppContainerSidFromAppContainerName(name, &sid);
    return SUCCEEDED(result) ? sid : nullptr;
}

std::vector<wchar_t> minimal_environment()
{
    wchar_t system_root[MAX_PATH] {};
    GetEnvironmentVariableW(L"SystemRoot", system_root, MAX_PATH);
    std::wstring value = L"SystemRoot=";
    value += system_root;
    std::vector<wchar_t> block(value.begin(), value.end());
    block.push_back(L'\0');
    block.push_back(L'\0');
    return block;
}

} // namespace

int wmain(int argc, wchar_t** argv)
{
    std::map<std::wstring, std::wstring> arguments;
    for (int index = 1; index + 1 < argc; index += 2)
        arguments.emplace(argv[index], argv[index + 1]);
    const bool generic = arguments.contains(L"--program");
    const std::vector<const wchar_t*> required_arguments = generic
        ? std::vector<const wchar_t*>{L"--program", L"--argument", L"--read", L"--memory-limit"}
        : std::vector<const wchar_t*>{L"--worker", L"--adapter", L"--component", L"--memory-limit"};
    for (const wchar_t* key : required_arguments) {
        if (!arguments.contains(key)) {
            std::wcerr << L"missing " << key << L'\n';
            return 2;
        }
    }
    uint64_t memory_limit = 0;
    const auto& memory_text = arguments[L"--memory-limit"];
    for (wchar_t value : memory_text) {
        if (value < L'0' || value > L'9') return 2;
        const uint64_t digit = static_cast<uint64_t>(value - L'0');
        if (memory_limit > (UINT64_MAX - digit) / 10) return 2;
        memory_limit = memory_limit * 10 + digit;
    }
    if (!memory_limit) return 2;

    PSID sid = app_container_sid();
    if (!sid) return 3;
    const std::wstring program = generic ? arguments[L"--program"] : arguments[L"--worker"];
    const std::wstring argument = generic ? arguments[L"--argument"] : arguments[L"--adapter"];
    grant_read_execute(program, sid);
    if (!generic) grant_read_execute(arguments[L"--adapter"], sid);
    const std::wstring readable = generic ? arguments[L"--read"] : arguments[L"--component"];
    if (!grant_read_execute(readable, sid)) {
        FreeSid(sid);
        return 4;
    }

    SIZE_T attribute_bytes = 0;
    InitializeProcThreadAttributeList(nullptr, 1, 0, &attribute_bytes);
    std::vector<std::byte> attribute_storage(attribute_bytes);
    auto* attributes = reinterpret_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(attribute_storage.data());
    if (!InitializeProcThreadAttributeList(attributes, 1, 0, &attribute_bytes)) {
        FreeSid(sid);
        return 5;
    }
    SECURITY_CAPABILITIES capabilities {};
    capabilities.AppContainerSid = sid;
    if (!UpdateProcThreadAttribute(
            attributes, 0, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
            &capabilities, sizeof(capabilities), nullptr, nullptr)) {
        DeleteProcThreadAttributeList(attributes);
        FreeSid(sid);
        return 5;
    }

    STARTUPINFOEXW startup {};
    startup.StartupInfo.cb = sizeof(startup);
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
    startup.StartupInfo.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
    startup.StartupInfo.hStdError = GetStdHandle(STD_ERROR_HANDLE);
    startup.lpAttributeList = attributes;
    std::wstring command = quote(program) + L" " + quote(argument);
    std::vector<wchar_t> command_line(command.begin(), command.end());
    command_line.push_back(L'\0');
    auto environment = minimal_environment();
    PROCESS_INFORMATION process {};
    const DWORD flags = EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT;
    const BOOL created = CreateProcessW(
        program.c_str(), command_line.data(), nullptr, nullptr, TRUE, flags,
        environment.data(), nullptr, &startup.StartupInfo, &process);
    DeleteProcThreadAttributeList(attributes);
    FreeSid(sid);
    if (!created) return 6;

    HANDLE job = CreateJobObjectW(nullptr, nullptr);
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits {};
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_ACTIVE_PROCESS |
        JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    limits.ProcessMemoryLimit = static_cast<SIZE_T>(memory_limit);
    if (!job || !SetInformationJobObject(job, JobObjectExtendedLimitInformation, &limits, sizeof(limits)) ||
        !AssignProcessToJobObject(job, process.hProcess)) {
        TerminateProcess(process.hProcess, 7);
        if (job) CloseHandle(job);
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
        return 7;
    }
    ResumeThread(process.hThread);
    CloseHandle(process.hThread);
    WaitForSingleObject(process.hProcess, INFINITE);
    DWORD exit_code = 8;
    GetExitCodeProcess(process.hProcess, &exit_code);
    CloseHandle(process.hProcess);
    CloseHandle(job);
    return static_cast<int>(exit_code);
}
