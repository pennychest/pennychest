import json

import pytest

from pennychest.ai.config import set_provider_values
from pennychest.core.config import settings
from tests.conftest import use_model
from tests.test_ai import api  # noqa: F401

# A layout the importer doesn't know by its column names
ODD = (
    "Posted,Ref,Who,Out,In,Running total\n"
    "20/09/2026,A1,TESCO STORES,12.50,,987.50\n"
    "21/09/2026,A2,SALARY,,1500.00,2487.50\n"
)
ODD_COLUMNS = {"date": 0, "description": 2, "money_out": 3, "money_in": 4}
JEV = "https://api.typesafe.ai/v1/systemone"


@pytest.fixture(autouse=True)
def uploads(tmp_path, monkeypatch):
    monkeypatch.setattr(settings, "upload_dir", str(tmp_path))


@pytest.fixture
def current(client):
    for path, type_ in [("Assets:Current", "asset"), ("Expenses:Uncategorised", "expense")]:
        client.post(
            "/api/accounts", json={"name": path.split(":")[-1], "full_path": path, "type": type_}
        )
    accounts = client.get("/api/accounts").json()
    return next(a["id"] for a in accounts if a["full_path"] == "Assets:Current")


def _file(content, name="statement.csv"):
    return {"file": (name, content.encode(), "text/csv")}


def _preview(client, content, **data):
    response = client.post("/api/imports/csv/preview", files=_file(content), data=data)
    assert response.status_code == 200, response.text
    return response.json()


def _jev_answers(columns, confidence=0.9):
    """Jev's reply choosing these columns, and "none" for the other fields."""
    fields = ("date", "description", "amount", "money_out", "money_in")
    return (200, {"answers": {
        f: {"type": "choice", "choice": str(columns[f]) if f in columns else "none",
            "confidence": confidence}
        for f in fields
    }})


def test_detect_offers_unrecognised_csvs_for_mapping(client):
    response = client.post("/api/imports/detect", files=_file(ODD))
    assert response.json() == {"importer": "csv", "statement": None}


def test_preview_maps_known_column_names(client):
    preview = _preview(client, "Date,Description,Amount\n20/09/2026,TESCO,-12.50\n")
    assert preview["headers"] == ["Date", "Description", "Amount"]
    assert preview["rows"] == [["20/09/2026", "TESCO", "-12.50"]]
    assert preview["columns"] == {
        "date": {"column": 0, "source": "name", "confidence": None},
        "description": {"column": 1, "source": "name", "confidence": None},
        "amount": {"column": 2, "source": "name", "confidence": None},
    }
    assert preview["template"] is None


def test_preview_without_a_model_leaves_the_gaps(client):
    preview = _preview(client, ODD)
    assert preview["columns"] == {}
    assert preview["ai_available"] is False


def test_a_decision_model_fills_the_gaps_with_its_confidence(client, api, db_session):
    set_provider_values(db_session, "typesafe", {"api_key": "ts-1"})
    use_model(db_session, "csv_columns", "typesafe", "jev-latest")
    api.on("POST", JEV, _jev_answers(ODD_COLUMNS, 0.88))

    preview = _preview(client, ODD)
    assert {f: c["column"] for f, c in preview["columns"].items()} == ODD_COLUMNS
    assert preview["columns"]["date"] == {"column": 0, "source": "ai", "confidence": 0.88}

    sent = api.body()
    assert sent["state"]["csv"]["headers"][0] == "Posted"
    assert sent["questions"]["date"]["criteria"]["5"] == "Column 6, headed 'Running total'"
    assert "none" in sent["questions"]["amount"]["criteria"]
    log = client.get("/api/ai/logs").json()["logs"][0]
    assert log["operation"] == "csv_columns" and log["error"] is None


def test_on_demand_the_model_waits_to_be_asked(client, api, db_session):
    set_provider_values(db_session, "typesafe", {"api_key": "ts-1"})
    use_model(db_session, "csv_columns", "typesafe", "jev-latest", mode="on_demand")
    assert _preview(client, ODD)["columns"] == {}
    assert api.requests == []

    api.on("POST", JEV, _jev_answers(ODD_COLUMNS))
    preview = _preview(client, ODD, ai="true")
    assert {f: c["column"] for f, c in preview["columns"].items()} == ODD_COLUMNS


def test_a_language_model_fills_the_gaps(client, api, db_session):
    set_provider_values(db_session, "openai", {"api_key": "sk-1"})
    use_model(db_session, "csv_columns", "openai", "gpt-5-mini")
    answer = {"date": 0, "description": 2, "amount": None, "money_out": 3, "money_in": 4}
    api.on("POST", "https://api.openai.com/v1/chat/completions", (200, {
        "choices": [{"message": {"content": json.dumps(answer)}, "finish_reason": "stop"}],
    }))
    preview = _preview(client, ODD)
    assert {f: c["column"] for f, c in preview["columns"].items()} == ODD_COLUMNS
    assert preview["columns"]["date"]["confidence"] is None


def test_column_names_win_over_the_model(client, api, db_session):
    set_provider_values(db_session, "typesafe", {"api_key": "ts-1"})
    use_model(db_session, "csv_columns", "typesafe", "jev-latest")
    # The names give the date and amount but not the description
    content = "Date,Who,Amount\n20/09/2026,TESCO,-12.50\n"
    api.on("POST", JEV, _jev_answers({"date": 2, "description": 1, "money_out": 2}))
    columns = _preview(client, content)["columns"]
    assert columns["date"]["source"] == "name" and columns["date"]["column"] == 0
    assert columns["description"] == {"column": 1, "source": "ai", "confidence": 0.9}
    assert "money_out" not in columns


def test_a_failing_model_is_reported(client, api, db_session):
    set_provider_values(db_session, "typesafe", {"api_key": "ts-1"})
    use_model(db_session, "csv_columns", "typesafe", "jev-latest")
    api.on("POST", JEV, (503, {"error": "overloaded"}))
    preview = _preview(client, ODD)
    assert preview["columns"] == {}
    assert preview["ai_error"]


def test_import_with_a_mapping(client, current):
    response = client.post(
        "/api/imports/upload",
        files=_file(ODD),
        data={"account_id": str(current), "importer_name": "csv",
              "csv_columns": json.dumps(ODD_COLUMNS)},
    )
    assert response.status_code == 200, response.text
    batch = client.get(f"/api/imports/batches/{response.json()['batch_id']}").json()
    amounts = {t["description"]: t["amount"] for t in batch["transactions"]}
    assert amounts == {"TESCO STORES": "-12.5000", "SALARY": "1500.0000"}


def test_money_out_is_outgoing_whichever_sign_the_bank_uses(client, current):
    content = "Posted,Who,Out,In\n20/09/2026,TESCO,-12.50,\n"
    columns = {"date": 0, "description": 1, "money_out": 2, "money_in": 3}
    response = client.post(
        "/api/imports/upload",
        files=_file(content),
        data={"account_id": str(current), "csv_columns": json.dumps(columns)},
    )
    batch = client.get(f"/api/imports/batches/{response.json()['batch_id']}").json()
    assert batch["transactions"][0]["amount"] == "-12.5000"


def test_import_needs_a_complete_mapping(client, current):
    response = client.post(
        "/api/imports/upload",
        files=_file(ODD),
        data={"account_id": str(current), "csv_columns": json.dumps({"date": 0})},
    )
    assert response.status_code == 400
    assert "description and amount" in response.json()["detail"]


def test_saved_templates_are_used_for_files_laid_out_the_same(client, api):
    headers = ODD.splitlines()[0].split(",")
    response = client.post(
        "/api/imports/csv/templates",
        json={"name": "Monzo export", "headers": headers, "columns": ODD_COLUMNS},
    )
    assert response.status_code == 201
    template_id = response.json()["id"]

    # Header case and spacing don't matter
    later = ODD.replace("Posted,", " posted ,").replace("21/09/2026", "22/09/2026")
    preview = _preview(client, later)
    assert preview["template"] == {"id": template_id, "name": "Monzo export"}
    assert preview["columns"]["description"] == {
        "column": 2, "source": "template", "confidence": None
    }
    assert api.requests == []

    assert [t["name"] for t in client.get("/api/imports/csv/templates").json()] == [
        "Monzo export"
    ]
    assert client.delete(f"/api/imports/csv/templates/{template_id}").status_code == 204
    assert _preview(client, later)["template"] is None


@pytest.mark.parametrize("body, problem", [
    ({"name": " ", "columns": ODD_COLUMNS}, "name"),
    ({"name": "X", "columns": {"date": 0}}, "description and amount"),
    ({"name": "X", "columns": {**ODD_COLUMNS, "date": 9}}, "outside the file"),
    ({"name": "X", "columns": {**ODD_COLUMNS, "balance": 5}}, "Unknown fields"),
])
def test_templates_are_checked(client, body, problem):
    headers = ODD.splitlines()[0].split(",")
    response = client.post("/api/imports/csv/templates", json={"headers": headers, **body})
    assert response.status_code == 400
    assert problem in response.json()["detail"]


def test_template_names_are_unique(client):
    headers = ODD.splitlines()[0].split(",")
    body = {"name": "Monzo", "headers": headers, "columns": ODD_COLUMNS}
    assert client.post("/api/imports/csv/templates", json=body).status_code == 201
    assert client.post("/api/imports/csv/templates", json=body).status_code == 400
