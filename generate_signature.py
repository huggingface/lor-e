import hashlib
import hmac


secret = "tmpsecret"
payload_body_string = '{"tmp":"bob"}'


def generate_signature(payload_body, secret_token):
    hash_object = hmac.new(
        secret_token.encode("utf-8"), msg=payload_body, digestmod=hashlib.sha256
    )
    expected_signature = "sha256=" + hash_object.hexdigest()
    return expected_signature


print(generate_signature(payload_body_string.encode("utf-8"), secret))
