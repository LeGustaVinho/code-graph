import os

from fastapi import Depends, FastAPI

from app.errors import ItemNotFound
from app.services import ItemService

app = FastAPI()

DATABASE_URL = os.environ.get("DATABASE_URL", "sqlite:///./app.db")
DEBUG = os.getenv("APP_DEBUG", "0") == "1"


def get_db():
    return {"url": DATABASE_URL}


@app.get("/api/items/{item_id}")
def read_item(item_id: int, db=Depends(get_db)):
    service = ItemService(db)
    return service.get(item_id)


@app.exception_handler(ItemNotFound)
def handle_not_found(request, exc):
    return {"error": str(exc)}


if __name__ == "__main__":
    print("run with uvicorn")
