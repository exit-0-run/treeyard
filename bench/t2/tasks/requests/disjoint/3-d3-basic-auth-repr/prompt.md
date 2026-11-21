# HTTPBasicAuth: repr without the password

In `src/requests/auth.py`, give `HTTPBasicAuth` a `repr` that does not show the password: `repr(HTTPBasicAuth("user", "pw"))` is `HTTPBasicAuth(username='user', password='***')`.

Keep the existing behaviour otherwise. Finish the work completely.
