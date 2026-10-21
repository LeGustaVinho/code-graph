from flask import Flask


def create_app():
    app = Flask(__name__)

    def load_user(token):
        return token

    @app.route("/profile")
    def profile():
        return load_user("t")

    return app
