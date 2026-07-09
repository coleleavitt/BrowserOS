#!/usr/bin/env python3
"""Tests for Windows signing path discovery."""

import unittest
from tempfile import TemporaryDirectory
from pathlib import Path
from types import SimpleNamespace
from typing import cast
from unittest import mock

from bos_build.core.context import Context
from bos_build.core.products import get_product_descriptor
from bos_build.lib.env import EnvConfig
from . import windows as windows_module
from .windows import (
    WindowsSignModule,
    get_browseros_server_binary_paths,
    get_existing_browseros_server_binary_paths,
    get_missing_required_browseros_server_binary_paths,
    sign_with_codesigntool,
)
class WindowsSignPathsTest(unittest.TestCase):
    def test_browseros_and_claw_server_binaries_are_expected_for_signing(self):
        build_output_dir = Path("/tmp/out/Default")

        self.assertEqual(
            get_browseros_server_binary_paths(build_output_dir),
            [
                build_output_dir
                / "BrowserOSServer"
                / "default"
                / "resources"
                / "bin"
                / "browseros_server.exe",
                build_output_dir
                / "BrowserClawServer"
                / "default"
                / "resources"
                / "bin"
                / "browseros-claw-server.exe",
            ],
        )

    def test_missing_optional_claw_binary_is_not_required_before_packaging(self):
        with TemporaryDirectory() as tmp:
            build_output_dir = Path(tmp)
            self._write_binary(
                build_output_dir
                / "BrowserOSServer"
                / "default"
                / "resources"
                / "bin"
                / "browseros_server.exe"
            )

            self.assertEqual(
                get_existing_browseros_server_binary_paths(build_output_dir),
                [
                    build_output_dir
                    / "BrowserOSServer"
                    / "default"
                    / "resources"
                    / "bin"
                    / "browseros_server.exe"
                ],
            )
            self.assertEqual(
                get_missing_required_browseros_server_binary_paths(build_output_dir),
                [],
            )

    def test_missing_claw_binary_is_required_once_root_is_packaged(self):
        with TemporaryDirectory() as tmp:
            build_output_dir = Path(tmp)
            self._write_binary(
                build_output_dir
                / "BrowserOSServer"
                / "default"
                / "resources"
                / "bin"
                / "browseros_server.exe"
            )
            (
                build_output_dir
                / "BrowserClawServer"
                / "default"
                / "resources"
                / "bin"
            ).mkdir(parents=True)

            self.assertEqual(
                get_missing_required_browseros_server_binary_paths(build_output_dir),
                [
                    build_output_dir
                    / "BrowserClawServer"
                    / "default"
                    / "resources"
                    / "bin"
                    / "browseros-claw-server.exe"
                ],
            )

    def test_sign_executables_fails_when_required_server_binary_missing(self):
        with TemporaryDirectory() as tmp:
            build_output_dir = Path(tmp)
            self._write_binary(build_output_dir / "chrome.exe")

            with self.assertRaisesRegex(RuntimeError, "browseros_server.exe"):
                WindowsSignModule()._sign_executables(
                    build_output_dir, self._ctx("browseros")
                )

    def test_missing_chrome_is_fatal_for_each_product(self):
        for product_id in ("browseros", "browserclaw"):
            with self.subTest(product=product_id), TemporaryDirectory() as tmp:
                build_output_dir = Path(tmp)
                for binary in get_browseros_server_binary_paths(
                    build_output_dir, product_id
                ):
                    self._write_binary(binary)

                with mock.patch(
                    "bos_build.steps.sign.windows.sign_with_codesigntool"
                ) as sign:
                    with self.assertRaisesRegex(
                        RuntimeError, "Missing primary browser executable:.*chrome.exe"
                    ):
                        WindowsSignModule()._sign_executables(
                            build_output_dir, self._ctx(product_id)
                        )

                sign.assert_not_called()

    def test_browserclaw_requires_claw_binary(self):
        with TemporaryDirectory() as tmp:
            build_output_dir = Path(tmp)
            self._write_binary(build_output_dir / "chrome.exe")

            with self.assertRaisesRegex(RuntimeError, "browseros-claw-server.exe"):
                WindowsSignModule()._sign_executables(
                    build_output_dir, self._ctx("browserclaw")
                )

    def _write_binary(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"binary")

    def _ctx(self, product: str):
        return cast(
            Context,
            SimpleNamespace(product=get_product_descriptor(product), env=mock.Mock()),
        )


class SignWithCodeSignToolInvocationTest(unittest.TestCase):
    def test_bat_invocation_uses_argv_list_and_redacted_logs(self):
        password = 'pa ss%"!^&word'
        totp_secret = "totp%secret!^&"
        credential_id = "credential id&123"

        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            tool_dir = root / "tool dir"
            tool_dir.mkdir()
            codesigntool = tool_dir / "CodeSignTool.bat"
            codesigntool.write_text("@echo off\n")

            binary_dir = root / "bin dir"
            binary_dir.mkdir()
            binary = binary_dir / "browser.exe"
            binary.write_bytes(b"binary")

            env = SimpleNamespace(
                code_sign_tool_exe=None,
                code_sign_tool_path=str(tool_dir),
                esigner_username="signer@example.com",
                esigner_password=password,
                esigner_totp_secret=totp_secret,
                esigner_credential_id=credential_id,
            )
            sign_result = SimpleNamespace(
                stdout=f"signed with {password} and {totp_secret}\n",
                stderr=f"using {credential_id}\n",
            )
            verify_result = SimpleNamespace(stdout="Valid\n", stderr="")
            info_logs: list[str] = []
            error_logs: list[str] = []

            with (
                mock.patch.object(
                    windows_module.subprocess,
                    "run",
                    side_effect=[sign_result, verify_result],
                ) as run,
                mock.patch.object(
                    windows_module, "log_info", side_effect=info_logs.append
                ),
                mock.patch.object(
                    windows_module, "log_success", side_effect=info_logs.append
                ),
                mock.patch.object(
                    windows_module, "log_error", side_effect=error_logs.append
                ),
            ):
                self.assertTrue(sign_with_codesigntool([binary], cast(EnvConfig, env)))

            sign_call = run.call_args_list[0]
            sign_cmd = sign_call.args[0]
            self.assertIsInstance(sign_cmd, list)
            self.assertEqual(sign_cmd[:3], ["cmd", "/c", str(codesigntool)])
            self.assertEqual(sign_call.kwargs["shell"], False)
            self.assertEqual(sign_call.kwargs["cwd"], str(tool_dir))
            self.assertEqual(sign_cmd[sign_cmd.index("-password") + 1], password)
            self.assertNotIn(f'"{password}"', sign_cmd)

            running_logs = [line for line in info_logs if line.startswith("Running:")]
            self.assertEqual(len(running_logs), 1)
            self.assertIn("-password ***", running_logs[0])
            self.assertIn("-totp_secret ***", running_logs[0])
            self.assertIn("-credential_id ***", running_logs[0])

            all_logs = "\n".join(info_logs + error_logs)
            self.assertNotIn(password, all_logs)
            self.assertNotIn(totp_secret, all_logs)
            self.assertNotIn(credential_id, all_logs)

    def test_code_sign_tool_exe_runs_directly_as_argv(self):
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            codesigntool = root / "CodeSignTool.sh"
            codesigntool.write_text("#!/bin/sh\n")
            binary = root / "browser.exe"
            binary.write_bytes(b"binary")

            env = SimpleNamespace(
                code_sign_tool_exe=str(codesigntool),
                code_sign_tool_path=None,
                esigner_username="signer@example.com",
                esigner_password="password",
                esigner_totp_secret="totp",
                esigner_credential_id=None,
            )
            sign_result = SimpleNamespace(stdout="", stderr="")
            verify_result = SimpleNamespace(stdout="Valid\n", stderr="")

            with (
                mock.patch.object(
                    windows_module.subprocess,
                    "run",
                    side_effect=[sign_result, verify_result],
                ) as run,
                mock.patch.object(windows_module, "log_info"),
                mock.patch.object(windows_module, "log_success"),
                mock.patch.object(windows_module, "log_error"),
            ):
                self.assertTrue(sign_with_codesigntool([binary], cast(EnvConfig, env)))

            sign_cmd = run.call_args_list[0].args[0]
            self.assertEqual(sign_cmd[0], str(codesigntool))
            self.assertNotEqual(sign_cmd[:3], ["cmd", "/c", str(codesigntool)])


if __name__ == "__main__":
    unittest.main()
