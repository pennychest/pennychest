"""Installing and removing plugins while PennyChest runs.

Plugins installed from Settings go into their own virtualenv in settings.plugin_dir, on the
same volume as the database, so they survive restarts and image updates. A .pth file lets the
virtualenv see PennyChest's own packages, so pip treats PennyChest and its dependencies as
already installed and adds only what a plugin needs on top. The virtualenv's site-packages is
appended to sys.path, after PennyChest's own, so a plugin can't replace a core dependency.

Each install is also recorded in app_settings. If the virtualenv is missing (a new volume) or
was made by another Python (an image update), restore() rebuilds it from those records.
"""

import importlib
import importlib.metadata
import json
import logging
import os
import re
import shutil
import site
import subprocess
import sys
import sysconfig
import threading
from pathlib import Path

from sqlalchemy.orm import Session

from pennychest.core.config import settings
from pennychest.settings.models import AppSetting

log = logging.getLogger(__name__)

# {package: pip requirement} for each plugin installed from Settings
INSTALLED_KEY = "plugins.installed"
HOST_PTH = "_pennychest_host.pth"
PIP_TIMEOUT_SECONDS = 600

# One pip at a time: two installs into the same virtualenv would trip over each other.
_lock = threading.Lock()


class PluginInstallError(Exception):
    pass


def canonical(package: str) -> str:
    """A package name as pip compares them (PEP 503)."""
    return re.sub(r"[-_.]+", "-", package).lower()


def _venv_path(name: str) -> Path:
    base = settings.plugin_dir
    return Path(sysconfig.get_path(name, scheme="venv", vars={"base": base, "platbase": base}))


def site_dir() -> Path:
    return _venv_path("purelib")


def _python() -> Path:
    return _venv_path("scripts") / ("python.exe" if os.name == "nt" else "python")


def _venv_is_current() -> bool:
    """Whether the virtualenv exists and was made by this Python."""
    cfg = Path(settings.plugin_dir) / "pyvenv.cfg"
    if not _python().exists() or not cfg.exists():
        return False
    version = re.search(r"^version(?:_info)?\s*=\s*(\d+\.\d+)", cfg.read_text(), re.MULTILINE)
    return version is not None and version[1] == f"{sys.version_info[0]}.{sys.version_info[1]}"


def _remove_venv() -> None:
    """Delete the plugin virtualenv, but nothing else that may be in plugin_dir by mistake."""
    path = Path(settings.plugin_dir)
    if (path / "pyvenv.cfg").exists():
        shutil.rmtree(path)
    elif path.exists() and any(path.iterdir()):
        raise PluginInstallError(f"{path} isn't empty and isn't a plugin virtualenv")


def _ensure_venv() -> None:
    if not _venv_is_current():
        _remove_venv()
        # Without pip: PennyChest's own pip installs into it with --python.
        subprocess.run(
            [sys.executable, "-m", "venv", "--without-pip", settings.plugin_dir],
            check=True,
            capture_output=True,
        )
    # Rewritten every time, in case PennyChest's own packages have moved.
    host_dirs = [d for d in site.getsitepackages() if Path(d) != site_dir()]
    site_dir().mkdir(parents=True, exist_ok=True)
    (site_dir() / HOST_PTH).write_text(
        "".join(f"import site; site.addsitedir({d!r})\n" for d in host_dirs)
    )


def _pip(*args: str) -> None:
    result = subprocess.run(
        [
            sys.executable, "-m", "pip", "--python", str(_python()),
            "--disable-pip-version-check", "--no-input", *args,
        ],
        capture_output=True,
        text=True,
        timeout=PIP_TIMEOUT_SECONDS,
    )
    if result.returncode != 0:
        lines = [line for line in result.stderr.splitlines() if line.strip()]
        errors = [line for line in lines if line.startswith("ERROR:")]
        raise PluginInstallError("\n".join(errors or lines[-5:]) or "pip failed")


def activate() -> None:
    """Make plugins installed from Settings importable. Called on startup."""
    if str(site_dir()) not in sys.path:
        site.addsitedir(str(site_dir()))
    importlib.invalidate_caches()


def installed() -> dict[str, str]:
    """{canonical package: version} for the plugins installed from Settings: the packages that
    register a PennyChest entry point, rather than the libraries they depend on."""
    return {
        canonical(dist.metadata["Name"]): dist.version
        for dist in importlib.metadata.distributions(path=[str(site_dir())])
        if any(ep.group.startswith("pennychest.") for ep in dist.entry_points)
    }


def built_in_version(package: str) -> str | None:
    """The version of a package that's part of PennyChest's own environment, e.g. a plugin
    built into the image, which can't be removed from Settings."""
    try:
        dist = importlib.metadata.distribution(package)
    except importlib.metadata.PackageNotFoundError:
        return None
    location = Path(str(dist.locate_file(""))).resolve()
    return None if location == site_dir().resolve() else dist.version


def _modules(package: str) -> set[str]:
    """The top-level modules a plugin installed, so they can be reloaded after it changes."""
    try:
        files = importlib.metadata.distribution(package).files or []
    except importlib.metadata.PackageNotFoundError:
        return set()
    return {
        f.parts[0].removesuffix(".py")
        for f in files
        if f.suffix == ".py" and not f.parts[0].endswith((".dist-info", ".data", ".."))
    }


def _forget(modules: set[str]) -> None:
    """Drop a plugin's modules, so the next plugin lookup imports the new version, or finds
    it gone."""
    for name in list(sys.modules):
        if name.split(".")[0] in modules:
            del sys.modules[name]
    importlib.invalidate_caches()


def _recorded(db: Session) -> dict[str, str]:
    setting = db.get(AppSetting, INSTALLED_KEY)
    return json.loads(setting.value) if setting and setting.value else {}


def _record(db: Session, recorded: dict[str, str]) -> None:
    setting = db.get(AppSetting, INSTALLED_KEY)
    value = json.dumps(recorded) if recorded else None
    if value is None:
        if setting:
            db.delete(setting)
    elif setting:
        setting.value = value
    else:
        db.add(AppSetting(key=INSTALLED_KEY, value=value))
    db.commit()


def install(db: Session, package: str, requirement: str) -> None:
    """Install or update a plugin from a pip requirement."""
    with _lock:
        modules = _modules(package)
        _ensure_venv()
        _pip("install", "--upgrade", requirement)
        _forget(modules)
        _record(db, _recorded(db) | {canonical(package): requirement})


def uninstall(db: Session, package: str) -> None:
    with _lock:
        modules = _modules(package)
        if canonical(package) in installed():
            _pip("uninstall", "--yes", package)
        _forget(modules)
        _record(db, {p: r for p, r in _recorded(db).items() if p != canonical(package)})
        # pip leaves a plugin's dependencies behind; once no plugins are left, so are they.
        if not installed():
            _remove_venv()
            importlib.invalidate_caches()


def _reinstall(recorded: dict[str, str]) -> None:
    with _lock:
        try:
            _ensure_venv()
        except Exception:
            log.exception("Couldn't create the plugin virtualenv in %s", settings.plugin_dir)
            return
        for package, requirement in recorded.items():
            if package in installed():
                continue
            try:
                _pip("install", requirement)
            except Exception:
                log.exception("Couldn't reinstall the %s plugin", package)
        importlib.invalidate_caches()


def restore(db: Session) -> None:
    """On startup, reinstall recorded plugins that are missing, in the background so
    PennyChest starts straight away."""
    recorded = _recorded(db)
    if not recorded:
        return
    if _venv_is_current() and recorded.keys() <= installed().keys():
        return
    threading.Thread(target=_reinstall, args=(recorded,), daemon=True).start()
