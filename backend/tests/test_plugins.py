import sys
import zipfile

import pytest

from pennychest.core.config import settings
from pennychest.export.base import get_exporter
from pennychest.plugins import installer, repositories
from pennychest.plugins.repositories import OFFICIAL, Index, IndexPlugin

FORK = "https://github.com/someone/pennychest-plugins"


def _index(*plugins: dict) -> Index:
    return Index(name="Test plugins", plugins=[IndexPlugin(**p) for p in plugins])


HSBC = {
    "package": "pennychest-hsbc",
    "name": "HSBC statements",
    "description": "Import HSBC PDF statements",
    "version": "0.1.0",
    "url": "https://example.com/hsbc.zip#subdirectory=hsbc",
}


@pytest.fixture
def indexes(monkeypatch):
    """{repository: Index} served instead of fetching plugins.json."""
    served: dict[str, Index] = {OFFICIAL: _index(HSBC)}

    def fetch(url):
        if url not in served:
            raise ValueError("not found")
        return served[url]

    monkeypatch.setattr(repositories, "fetch_index", fetch)
    return served


@pytest.fixture
def plugin_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "plugin_dir", str(tmp_path / "plugins"))
    monkeypatch.setattr(sys, "path", list(sys.path))
    installer.activate()
    return tmp_path / "plugins"


# Repository links


@pytest.mark.parametrize(
    "repository, expected",
    [
        (OFFICIAL, "https://raw.githubusercontent.com/pennychest/pennychest-plugins/HEAD/plugins.json"),
        ("https://example.com/plugins", "https://example.com/plugins/plugins.json"),
        ("https://example.com/my-index.json", "https://example.com/my-index.json"),
    ],
)
def test_index_url(repository, expected):
    assert repositories.index_url(repository) == expected


def test_normalise_strips_trailing_slash_and_git():
    assert repositories.normalise(f" {FORK}.git/ ") == FORK


def test_normalise_requires_https():
    with pytest.raises(ValueError):
        repositories.normalise("http://github.com/someone/plugins")


def test_index_rejects_plugins_that_arent_https():
    with pytest.raises(ValueError):
        IndexPlugin(**HSBC | {"url": "file:///etc/passwd"})


def test_index_rejects_pip_options_as_package_names():
    with pytest.raises(ValueError):
        IndexPlugin(**HSBC | {"package": "--index-url=https://evil.example"})


# API


def test_lists_the_official_repository(client, indexes, plugin_dir):
    body = client.get("/api/plugins").json()
    [official] = body["repositories"]
    assert official["official"] is True
    assert official["name"] == "Test plugins"
    assert official["plugins"] == [
        {
            "package": "pennychest-hsbc",
            "name": "HSBC statements",
            "description": "Import HSBC PDF statements",
            "version": "0.1.0",
            "installed_version": None,
            "built_in": False,
        }
    ]
    assert body["can_add_repositories"] is True


def test_unreadable_repository_is_listed_with_an_error(client, indexes, plugin_dir):
    del indexes[OFFICIAL]
    [official] = client.get("/api/plugins").json()["repositories"]
    assert official["error"]
    assert official["plugins"] == []


def test_adds_and_removes_a_repository(client, indexes, plugin_dir):
    indexes[FORK] = _index()
    assert client.post("/api/plugins/repositories", json={"url": FORK + "/"}).status_code == 204
    urls = [r["url"] for r in client.get("/api/plugins").json()["repositories"]]
    assert urls == [OFFICIAL, FORK]

    assert client.delete("/api/plugins/repositories", params={"url": FORK}).status_code == 204
    urls = [r["url"] for r in client.get("/api/plugins").json()["repositories"]]
    assert urls == [OFFICIAL]


def test_wont_add_a_repository_twice(client, indexes, plugin_dir):
    response = client.post("/api/plugins/repositories", json={"url": OFFICIAL})
    assert response.status_code == 400


def test_wont_add_a_repository_without_an_index(client, indexes, plugin_dir):
    response = client.post("/api/plugins/repositories", json={"url": FORK})
    assert response.status_code == 400
    assert "plugins.json" in response.json()["detail"]


def test_adding_repositories_can_be_turned_off(client, indexes, plugin_dir, monkeypatch):
    indexes[FORK] = _index()
    client.post("/api/plugins/repositories", json={"url": FORK})
    monkeypatch.setattr(settings, "allow_plugin_repositories", False)

    assert client.post("/api/plugins/repositories", json={"url": FORK}).status_code == 403
    body = client.get("/api/plugins").json()
    assert [r["url"] for r in body["repositories"]] == [OFFICIAL]
    assert body["can_add_repositories"] is False


def test_installs_from_the_url_the_repository_lists(client, indexes, plugin_dir, monkeypatch):
    calls = []
    monkeypatch.setattr(installer, "install", lambda db, package, req: calls.append((package, req)))
    response = client.post(
        "/api/plugins/install", json={"repository": OFFICIAL, "package": "pennychest-hsbc"}
    )
    assert response.status_code == 204
    assert calls == [("pennychest-hsbc", f"pennychest-hsbc @ {HSBC['url']}")]


def test_wont_install_from_a_repository_that_isnt_added(client, indexes, plugin_dir):
    indexes[FORK] = _index(HSBC)
    response = client.post(
        "/api/plugins/install", json={"repository": FORK, "package": "pennychest-hsbc"}
    )
    assert response.status_code == 400


def test_wont_install_a_plugin_the_repository_doesnt_list(client, indexes, plugin_dir):
    response = client.post(
        "/api/plugins/install", json={"repository": OFFICIAL, "package": "pennychest-other"}
    )
    assert response.status_code == 404


def test_plugins_need_signing_in(anon_client, indexes, plugin_dir):
    assert anon_client.get("/api/plugins").status_code == 401
    response = anon_client.post(
        "/api/plugins/install", json={"repository": OFFICIAL, "package": "pennychest-hsbc"}
    )
    assert response.status_code == 401


# Installing for real, from a wheel built here so no network is needed


def _wheel(directory, version: str):
    """A plugin wheel with an exporter whose output is its version."""
    dist = f"pennychest_testplugin-{version}"
    files = {
        "pennychest_testplugin/__init__.py": (
            "from pennychest.export.base import BaseExporter\n"
            "class Exporter(BaseExporter):\n"
            "    name = 'testplugin'\n"
            "    file_extension = 'txt'\n"
            f"    def export(self, ledger): return {version!r}\n"
        ),
        f"{dist}.dist-info/METADATA": (
            f"Metadata-Version: 2.1\nName: pennychest-testplugin\nVersion: {version}\n"
        ),
        f"{dist}.dist-info/WHEEL": (
            "Wheel-Version: 1.0\nGenerator: test\nRoot-Is-Purelib: true\nTag: py3-none-any\n"
        ),
        f"{dist}.dist-info/entry_points.txt": (
            "[pennychest.exporters]\ntestplugin = pennychest_testplugin:Exporter\n"
        ),
    }
    record = f"{dist}.dist-info/RECORD"
    path = directory / f"{dist}-py3-none-any.whl"
    with zipfile.ZipFile(path, "w") as wheel:
        for name, content in files.items():
            wheel.writestr(name, content)
        wheel.writestr(record, "".join(f"{name},,\n" for name in [*files, record]))
    return f"pennychest-testplugin @ {path.as_uri()}"


def test_installs_updates_and_removes_a_plugin(db_session, plugin_dir, tmp_path):
    installer.install(db_session, "pennychest-testplugin", _wheel(tmp_path, "1.0"))
    assert installer.installed() == {"pennychest-testplugin": "1.0"}
    assert get_exporter("testplugin").export(None) == "1.0"

    # Picked up straight away, without a restart
    installer.install(db_session, "pennychest-testplugin", _wheel(tmp_path, "2.0"))
    assert get_exporter("testplugin").export(None) == "2.0"

    installer.uninstall(db_session, "pennychest-testplugin")
    assert get_exporter("testplugin") is None
    assert installer.installed() == {}
    assert not plugin_dir.exists()


def test_reinstalls_recorded_plugins_when_the_virtualenv_is_gone(db_session, plugin_dir, tmp_path):
    installer.install(db_session, "pennychest-testplugin", _wheel(tmp_path, "1.0"))
    installer._remove_venv()
    assert installer.installed() == {}

    installer._reinstall(installer._recorded(db_session))
    assert installer.installed() == {"pennychest-testplugin": "1.0"}
    installer.uninstall(db_session, "pennychest-testplugin")


def test_wont_delete_a_plugin_dir_that_isnt_a_virtualenv(db_session, plugin_dir, tmp_path):
    plugin_dir.mkdir()
    (plugin_dir / "pennychest.db").write_text("precious")
    with pytest.raises(installer.PluginInstallError):
        installer.install(db_session, "pennychest-testplugin", _wheel(tmp_path, "1.0"))
    assert (plugin_dir / "pennychest.db").read_text() == "precious"
