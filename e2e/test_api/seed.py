from test_api.models import Owner, Pet, PetStatus


def seed_pets() -> dict[int, Pet]:
    pets = [
        Pet(id=1, name="Fido", status=PetStatus.active, tag="dog", owner_id=1),
        Pet(id=2, name="Whiskers", status=PetStatus.adopted, tag="cat", owner_id=1),
        Pet(id=3, name="Buddy", status=PetStatus.active, tag="dog", owner_id=2),
        Pet(id=4, name="Luna", status=PetStatus.pending, tag="cat"),
    ]
    return {p.id: p for p in pets}


def seed_owners() -> dict[int, Owner]:
    owners = [
        Owner(id=1, name="Alice", email="alice@example.com"),
        Owner(id=2, name="Bob", email="bob@example.com"),
    ]
    return {o.id: o for o in owners}
