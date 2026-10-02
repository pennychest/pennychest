from pydantic import BaseModel


class RuleCreate(BaseModel):
    pattern: str
    match_type_id: int
    target_account_id: int
    priority: int = 0
    source: str = "manual"
    description: str | None = None


class RuleUpdate(BaseModel):
    pattern: str | None = None
    match_type_id: int | None = None
    target_account_id: int | None = None
    priority: int | None = None
    description: str | None = None


class RuleResponse(BaseModel):
    id: int
    pattern: str
    match_type_id: int
    match_type_name: str | None = None
    target_account_id: int
    target_account_path: str | None = None
    priority: int
    source: str | None = None
    description: str | None = None

    model_config = {"from_attributes": True}


class RulePreviewRequest(BaseModel):
    pattern: str
    match_type_id: int
    exclude_rule_id: int | None = None


class RuleConflict(BaseModel):
    rule_id: int
    pattern: str
    match_type: str
    priority: int
    target_account_id: int


class RulePreviewResponse(BaseModel):
    matching_transactions: list[dict]
    match_count: int
    conflicts: list[RuleConflict]


class RuleTestRequest(BaseModel):
    pattern: str
    match_type_id: int
    descriptions: list[str]


class RuleTestResponse(BaseModel):
    matching: list[str]
    match_count: int
    total: int


class RuleApplyPreviewItem(BaseModel):
    transaction_id: int
    description: str
    current_account_path: str | None
    new_account_path: str | None
    rule_id: int
    rule_pattern: str
    is_manual: bool
    would_change: bool


class RuleApplyPreview(BaseModel):
    items: list[RuleApplyPreviewItem]
    would_change_count: int
    manual_conflict_count: int
