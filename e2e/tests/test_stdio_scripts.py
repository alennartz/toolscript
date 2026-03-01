import json
import pytest
from mcp import ClientSession


def parse_result(result) -> dict:
    """Parse a successful execute_script response.

    Returns the parsed JSON containing 'result', 'logs', and 'stats'.
    Raises AssertionError if the response indicates an error.
    """
    text = result.content[0].text
    assert not result.isError, f"Script execution error: {text}"
    return json.loads(text)


@pytest.mark.asyncio
async def test_list_pets_smoke(mcp_stdio_session: ClientSession):
    """Smoke test: verify response format from execute_script."""
    result = await mcp_stdio_session.call_tool("execute_script", {
        "script": "return sdk.list_pets()"
    })
    data = parse_result(result)
    assert "result" in data
    assert "logs" in data


@pytest.mark.asyncio
async def test_list_pets(mcp_stdio_session: ClientSession):
    """sdk.list_pets() should return seeded data with items and total."""
    result = await mcp_stdio_session.call_tool("execute_script", {
        "script": "return sdk.list_pets()"
    })
    data = parse_result(result)
    pets = data["result"]
    assert pets["total"] == 4
    assert len(pets["items"]) == 4
    names = {p["name"] for p in pets["items"]}
    assert "Fido" in names
    assert "Whiskers" in names
    assert "Buddy" in names
    assert "Luna" in names


@pytest.mark.asyncio
async def test_get_pet_by_id(mcp_stdio_session: ClientSession):
    """sdk.get_pet({ pet_id = 1 }) should return Fido."""
    result = await mcp_stdio_session.call_tool("execute_script", {
        "script": "return sdk.get_pet({ pet_id = 1 })"
    })
    data = parse_result(result)
    pet = data["result"]
    assert pet["id"] == 1
    assert pet["name"] == "Fido"
    assert pet["status"] == "active"
    assert pet["tag"] == "dog"
    assert pet["owner_id"] == 1


@pytest.mark.asyncio
async def test_create_pet(mcp_stdio_session: ClientSession):
    """sdk.create_pet({...}) should create a new pet and return it."""
    result = await mcp_stdio_session.call_tool("execute_script", {
        "script": 'return sdk.create_pet({ name = "Spark", status = "active", tag = "hamster" })'
    })
    data = parse_result(result)
    pet = data["result"]
    assert pet["name"] == "Spark"
    assert pet["status"] == "active"
    assert pet["tag"] == "hamster"
    assert "id" in pet


@pytest.mark.asyncio
async def test_update_pet(mcp_stdio_session: ClientSession):
    """sdk.update_pet({ pet_id = 1 }, body) should update and return the pet."""
    result = await mcp_stdio_session.call_tool("execute_script", {
        "script": 'return sdk.update_pet({ pet_id = 1 }, { name = "Fido Jr." })'
    })
    data = parse_result(result)
    pet = data["result"]
    assert pet["id"] == 1
    assert pet["name"] == "Fido Jr."


@pytest.mark.asyncio
async def test_delete_pet(mcp_stdio_session: ClientSession):
    """sdk.delete_pet({ pet_id = 1 }) should delete the pet."""
    result = await mcp_stdio_session.call_tool("execute_script", {
        "script": "return sdk.delete_pet({ pet_id = 1 })"
    })
    data = parse_result(result)
    assert data["result"]["status"] == "deleted"


@pytest.mark.asyncio
async def test_query_params(mcp_stdio_session: ClientSession):
    """sdk.list_pets({ limit = 2, status = "active" }) should filter by query params."""
    result = await mcp_stdio_session.call_tool("execute_script", {
        "script": 'return sdk.list_pets({ limit = 2, status = "active" })'
    })
    data = parse_result(result)
    pets = data["result"]
    # There are 2 active pets (Fido, Buddy), limit=2 should return both
    assert len(pets["items"]) <= 2
    for p in pets["items"]:
        assert p["status"] == "active"


@pytest.mark.asyncio
async def test_nested_resource(mcp_stdio_session: ClientSession):
    """sdk.list_owner_pets({ owner_id = 1 }) should return pets for a specific owner."""
    result = await mcp_stdio_session.call_tool("execute_script", {
        "script": "return sdk.list_owner_pets({ owner_id = 1 })"
    })
    data = parse_result(result)
    pets = data["result"]
    # Owner 1 (Alice) has Fido and Whiskers
    assert len(pets) == 2
    names = {p["name"] for p in pets}
    assert "Fido" in names
    assert "Whiskers" in names


@pytest.mark.asyncio
async def test_multi_call_script(mcp_stdio_session: ClientSession):
    """Chain: list_pets -> get first pet by ID from result."""
    script = """
        local all_pets = sdk.list_pets()
        local first_id = all_pets.items[1].id
        local detail = sdk.get_pet({ pet_id = first_id })
        return { list_count = all_pets.total, detail = detail }
    """
    result = await mcp_stdio_session.call_tool("execute_script", {
        "script": script
    })
    data = parse_result(result)
    r = data["result"]
    assert r["list_count"] == 4
    assert "detail" in r
    assert r["detail"]["id"] is not None
    assert r["detail"]["name"] is not None


@pytest.mark.asyncio
async def test_create_then_fetch(mcp_stdio_session: ClientSession):
    """Chain: create pet -> fetch it by returned ID."""
    script = """
        local created = sdk.create_pet({ name = "Ziggy", status = "pending", tag = "parrot" })
        local fetched = sdk.get_pet({ pet_id = created.id })
        return { created = created, fetched = fetched }
    """
    result = await mcp_stdio_session.call_tool("execute_script", {
        "script": script
    })
    data = parse_result(result)
    r = data["result"]
    assert r["created"]["name"] == "Ziggy"
    assert r["fetched"]["name"] == "Ziggy"
    assert r["created"]["id"] == r["fetched"]["id"]


@pytest.mark.asyncio
async def test_enum_values(mcp_stdio_session: ClientSession):
    """sdk.list_pets({ status = "pending" }) -> all returned pets should have status "pending"."""
    result = await mcp_stdio_session.call_tool("execute_script", {
        "script": 'return sdk.list_pets({ status = "pending" })'
    })
    data = parse_result(result)
    pets = data["result"]
    assert pets["total"] >= 1
    for p in pets["items"]:
        assert p["status"] == "pending"


@pytest.mark.asyncio
async def test_optional_fields(mcp_stdio_session: ClientSession):
    """sdk.get_pet({ pet_id = 4 }) -> Luna has no owner_id (should be null)."""
    result = await mcp_stdio_session.call_tool("execute_script", {
        "script": "return sdk.get_pet({ pet_id = 4 })"
    })
    data = parse_result(result)
    pet = data["result"]
    assert pet["name"] == "Luna"
    assert pet["owner_id"] is None


@pytest.mark.asyncio
async def test_script_error_handling(mcp_stdio_session: ClientSession):
    """sdk.get_pet({ pet_id = 9999 }) -> should get a 404 error, not crash."""
    result = await mcp_stdio_session.call_tool("execute_script", {
        "script": "return sdk.get_pet({ pet_id = 9999 })"
    })
    # The response should indicate an error
    assert result.isError is True
    text = result.content[0].text
    assert "404" in text or "not found" in text.lower() or "error" in text.lower()


@pytest.mark.asyncio
async def test_script_timeout(mcp_limited_session: ClientSession):
    """An infinite loop is killed after the timeout."""
    result = await mcp_limited_session.call_tool("execute_script", {
        "script": "while true do end"
    })
    assert result.isError is True
    text = result.content[0].text
    assert "timeout" in text.lower() or "time" in text.lower() or "interrupt" in text.lower()


@pytest.mark.asyncio
async def test_max_api_calls_exceeded(mcp_limited_session: ClientSession):
    """Script making more than 3 API calls is stopped."""
    result = await mcp_limited_session.call_tool("execute_script", {
        "script": """
            for i = 1, 10 do
                sdk.list_pets()
            end
            return "should not reach here"
        """
    })
    assert result.isError is True
    text = result.content[0].text
    assert "api" in text.lower() or "limit" in text.lower() or "exceeded" in text.lower() or "call" in text.lower()


@pytest.mark.asyncio
async def test_sandbox_no_file_io(mcp_stdio_session: ClientSession):
    """io.open() is blocked by the Luau sandbox."""
    result = await mcp_stdio_session.call_tool("execute_script", {
        "script": 'local f = io.open("/etc/passwd", "r"); return f'
    })
    assert result.isError is True
    text = result.content[0].text
    assert "error" in text.lower() or "nil" in text.lower() or "attempt to index" in text.lower() or "io" in text.lower()


@pytest.mark.asyncio
async def test_file_save_writes_to_disk(mcp_output_session):
    """file.save() should write a file and report it in files_written."""
    session, output_dir = mcp_output_session
    result = await session.call_tool("execute_script", {
        "script": '''
            local pets = sdk.list_pets()
            local csv = "id,name\\n"
            for _, p in ipairs(pets.items) do
                csv = csv .. p.id .. "," .. p.name .. "\\n"
            end
            file.save("pets.csv", csv)
            return { saved = true, count = #pets.items }
        '''
    })
    data = parse_result(result)
    assert data["result"]["saved"] is True
    assert data["result"]["count"] == 4

    # Check files_written in response
    assert "files_written" in data
    assert len(data["files_written"]) == 1
    assert data["files_written"][0]["name"] == "pets.csv"
    assert data["files_written"][0]["bytes"] > 0

    # Verify file on disk
    csv_path = output_dir / "pets.csv"
    assert csv_path.exists()
    content = csv_path.read_text()
    assert "Fido" in content
    assert "Whiskers" in content


@pytest.mark.asyncio
async def test_file_save_rejects_traversal(mcp_output_session):
    """file.save() should reject path traversal attempts."""
    session, _ = mcp_output_session
    result = await session.call_tool("execute_script", {
        "script": 'return file.save("../evil.txt", "pwned")'
    })
    assert result.isError is True
    text = result.content[0].text
    assert "traversal" in text.lower() or "error" in text.lower()
