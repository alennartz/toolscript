from enum import Enum
from pydantic import BaseModel


class PetStatus(str, Enum):
    active = "active"
    adopted = "adopted"
    pending = "pending"


class Pet(BaseModel):
    id: int
    name: str
    status: PetStatus
    tag: str | None = None
    owner_id: int | None = None


class PetCreate(BaseModel):
    name: str
    status: PetStatus = PetStatus.active
    tag: str | None = None
    owner_id: int | None = None


class PetUpdate(BaseModel):
    name: str | None = None
    status: PetStatus | None = None
    tag: str | None = None
    owner_id: int | None = None


class PetList(BaseModel):
    items: list[Pet]
    total: int


class Owner(BaseModel):
    id: int
    name: str
    email: str
