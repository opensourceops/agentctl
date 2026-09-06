ARG PYTHON_BASE
FROM ${PYTHON_BASE}
WORKDIR /app
COPY requirements.lock /app/requirements.lock
RUN python -m pip install --no-cache-dir --only-binary=:all: --require-hashes -r requirements.lock
COPY app.py /app/app.py
COPY tests/test_app.py /app/test_app.py
USER 65532:65532
ENV PYTHONDONTWRITEBYTECODE=1
ENTRYPOINT ["python", "/app/app.py"]
